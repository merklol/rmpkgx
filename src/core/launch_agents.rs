//! Read-only association of per-user LaunchAgents. Never runs programs or
//! launchctl, expands shell syntax, resolves PATH, or follows executable links.
use super::{
    application::Application,
    association::{associate, Confidence},
    containers::Evidence,
};
use anyhow::{ensure, Context};
use std::path::{Component, Path};

pub fn evaluate(app: &Application, path: &Path) -> anyhow::Result<Option<Evidence>> {
    if path
        .extension()
        .is_none_or(|extension| extension != "plist")
    {
        return Ok(None);
    }
    let value = super::plist_io::read_plist(path)?;
    let dict = value
        .as_dictionary()
        .context("LaunchAgent plist is not a dictionary")?;
    let label = dict
        .get("Label")
        .and_then(plist::Value::as_string)
        .filter(|s| !s.is_empty())
        .context("LaunchAgent requires a nonempty string Label")?;
    let label_match = label == app.bundle_id
        || label
            .strip_prefix(&app.bundle_id)
            .is_some_and(|suffix| suffix.starts_with('.'));
    let name_match = path
        .file_name()
        .and_then(|name| associate(app, &name.to_string_lossy()))
        .is_some();
    let program = dict
        .get("Program")
        .map(|value| {
            value
                .as_string()
                .filter(|s| !s.is_empty())
                .context("invalid Program value")
        })
        .transpose()?;
    let args = dict
        .get("ProgramArguments")
        .map(|value| {
            let array = value
                .as_array()
                .context("ProgramArguments must be an array")?;
            ensure!(!array.is_empty(), "ProgramArguments must not be empty");
            array
                .iter()
                .map(|v| {
                    v.as_string()
                        .context("ProgramArguments entries must be strings")
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .transpose()?;
    let (executable, key) = if let Some(program) = program {
        (Some(program), "Program")
    } else if dict.contains_key("BundleProgram") {
        (None, "BundleProgram (SMAppService context unavailable)")
    } else {
        (
            args.as_ref().and_then(|args| args.first().copied()),
            "ProgramArguments[0]",
        )
    };
    if let Some(executable) = executable {
        let target = Path::new(executable);
        let literal = target.is_absolute()
            && !target
                .components()
                .any(|c| matches!(c, Component::ParentDir));
        if literal && target.starts_with(&app.path) && target != app.path {
            match super::plist_io::regular_file(target) {
                Ok(_) => return Ok(Some(Evidence { confidence: Confidence::High, reason: format!("LaunchAgent Label={label}; {key} directly names regular file {} inside the selected bundle; loaded service state is unknown", target.display()), shared: false, issue: None })),
                Err(error) => return Ok(Some(Evidence { confidence: Confidence::Medium, reason: format!("LaunchAgent Label={label}; {key} names a path inside the selected bundle, but it cannot be verified without following links: {error:#}; inspect only"), shared: false, issue: Some(format!("LaunchAgent executable cannot be verified: {error:#}")) })),
            }
        }
    }
    if label_match || name_match {
        return Ok(Some(Evidence { confidence: Confidence::Medium, reason: format!("LaunchAgent Label={label}; label/filename association only; {key} has no verified direct executable inside the selected bundle; wrappers, PATH and shell arguments are not interpreted; inspect only"), shared: false, issue: None }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (
        tempfile::TempDir,
        Application,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let app = Application {
            name: "Fixture".into(),
            bundle_id: "org.fixture.app".into(),
            path: root.join("Fixture.app"),
            version: None,
            protected: false,
        };
        let executable = app.path.join("Contents/MacOS/helper");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "fixture").unwrap();
        let path = root.join("LaunchAgents/unrelated.plist");
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        (temp, app, executable, path)
    }
    fn write(path: &Path, program: Option<&str>, arguments: &[&str], label: &str, binary: bool) {
        let mut dict = plist::Dictionary::new();
        dict.insert("Label".into(), label.into());
        if let Some(program) = program {
            dict.insert("Program".into(), program.into());
        }
        if !arguments.is_empty() {
            dict.insert(
                "ProgramArguments".into(),
                plist::Value::Array(arguments.iter().map(|s| (*s).into()).collect()),
            );
        }
        let value = plist::Value::Dictionary(dict);
        if binary {
            value.to_file_binary(path).unwrap();
        } else {
            value.to_file_xml(path).unwrap();
        }
    }
    #[test]
    fn executable_inside_bundle_matches_even_an_unrelated_plist_name_and_label() {
        let (_temp, app, executable, path) = fixture();
        for binary in [false, true] {
            write(
                &path,
                None,
                &[executable.to_str().unwrap(), "--background"],
                "unrelated.agent",
                binary,
            );
            let evidence = evaluate(&app, &path).unwrap().unwrap();
            assert_eq!(evidence.confidence, Confidence::High);
            assert!(evidence.reason.contains("ProgramArguments[0]"));
        }
    }
    #[test]
    fn explicit_program_overrides_argv_zero_and_arguments_are_not_ownership_evidence() {
        let (_temp, app, executable, path) = fixture();
        let executable = executable.to_str().unwrap();
        write(
            &path,
            Some("/usr/bin/true"),
            &[executable],
            "unrelated.agent",
            false,
        );
        assert!(evaluate(&app, &path).unwrap().is_none());
        write(
            &path,
            Some(executable),
            &["arbitrary argv zero"],
            "unrelated.agent",
            false,
        );
        assert_eq!(
            evaluate(&app, &path).unwrap().unwrap().confidence,
            Confidence::High
        );
        write(
            &path,
            None,
            &["/bin/sh", "-c", executable],
            "unrelated.agent",
            false,
        );
        assert!(evaluate(&app, &path).unwrap().is_none());
    }
    #[test]
    fn labels_are_weak_and_relative_or_sibling_paths_are_not_promoted() {
        let (_temp, app, executable, path) = fixture();
        write(&path, None, &["helper"], "org.fixture.app.agent", false);
        assert_eq!(
            evaluate(&app, &path).unwrap().unwrap().confidence,
            Confidence::Medium
        );
        let sibling = executable
            .to_string_lossy()
            .replace("Fixture.app/", "Fixture.app.backup/");
        write(&path, None, &[&sibling], "unrelated.agent", false);
        assert!(evaluate(&app, &path).unwrap().is_none());
        write(
            &path,
            None,
            &["/bin/sh", "-c", executable.to_str().unwrap()],
            "org.fixture.application",
            false,
        );
        assert!(evaluate(&app, &path).unwrap().is_none());
    }
    #[test]
    fn symlinked_program_and_plist_never_provide_verified_ownership() {
        let (temp, app, executable, path) = fixture();
        let link = app.path.join("Contents/MacOS/link");
        std::os::unix::fs::symlink(&executable, &link).unwrap();
        write(
            &path,
            Some(link.to_str().unwrap()),
            &[],
            "unrelated.agent",
            false,
        );
        let evidence = evaluate(&app, &path).unwrap().unwrap();
        assert_eq!(evidence.confidence, Confidence::Medium);
        assert!(evidence.issue.is_some());
        let alias = temp.path().canonicalize().unwrap().join("alias.plist");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(evaluate(&app, &alias).is_err());
    }
    #[test]
    fn invalid_arguments_oversized_plists_and_bundle_relative_context_are_conservative() {
        let (_temp, app, _executable, path) = fixture();
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(evaluate(&app, &path).is_err());
        let mut dict = plist::Dictionary::new();
        dict.insert("Label".into(), "org.fixture.app.agent".into());
        dict.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![3u64.into()]),
        );
        plist::Value::Dictionary(dict.clone())
            .to_file_xml(&path)
            .unwrap();
        assert!(evaluate(&app, &path).is_err());
        dict.remove("ProgramArguments");
        dict.insert("BundleProgram".into(), "Contents/MacOS/helper".into());
        plist::Value::Dictionary(dict).to_file_xml(&path).unwrap();
        assert_eq!(
            evaluate(&app, &path).unwrap().unwrap().confidence,
            Confidence::Medium
        );
    }
}
