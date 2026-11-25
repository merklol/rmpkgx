//! Read-only candidates. Absence from discovery is never proof of abandonment.
use super::{application::Application, discovery, disk_usage, plist_io, Issue, Report};
use crate::error::Error;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Classification {
    Verified,
    Likely,
    Unknown,
}
#[derive(Debug, Serialize)]
pub struct Candidate {
    pub path: PathBuf,
    pub bundle_id: String,
    pub bytes: u64,
    pub size_complete: bool,
    pub classification: Classification,
    pub evidence: String,
    pub shared: bool,
}
#[derive(Debug, Serialize)]
pub struct OrphanReport {
    pub findings: Vec<Candidate>,
    pub issues: Vec<Issue>,
    pub read_only: bool,
    pub limitation: &'static str,
}
impl OrphanReport {
    pub fn text(&self) -> String {
        let mut text = format!(
            "Possible leftovers · read-only\n{} candidates · {} inspection issues\n{}\n",
            self.findings.len(),
            self.issues.len(),
            self.limitation
        );
        for f in &self.findings {
            text.push_str(&format!(
                "\n{:?} · {} bytes{} · {}{}\n  {}\n  {}\n",
                f.classification,
                f.bytes,
                if f.size_complete { "" } else { " (incomplete)" },
                f.bundle_id,
                if f.shared {
                    " · shared/unknown ownership"
                } else {
                    ""
                },
                f.path.display(),
                f.evidence
            ));
        }
        for issue in &self.issues {
            text.push_str(&format!(
                "\nUninspected: {}: {}\n",
                issue.path.display(),
                issue.message
            ));
        }
        text
    }
}
fn identifier(name: &str) -> bool {
    let parts: Vec<_> = name.split('.').collect();
    parts.len() >= 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}
fn related(id: &str, installed: &str) -> bool {
    id.eq_ignore_ascii_case(installed)
        || id
            .to_ascii_lowercase()
            .starts_with(&format!("{}.", installed.to_ascii_lowercase()))
        || installed
            .to_ascii_lowercase()
            .starts_with(&format!("{}.", id.to_ascii_lowercase()))
}
// Read-only traversal still rejects symlinked ancestors and relative paths.
fn inspectable(path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(path.is_absolute(), "absolute scan root required");
    let mut prefix = PathBuf::new();
    for c in path.components() {
        anyhow::ensure!(
            !matches!(c, std::path::Component::ParentDir),
            "parent traversal rejected"
        );
        prefix.push(c);
        anyhow::ensure!(
            !std::fs::symlink_metadata(&prefix)?.file_type().is_symlink(),
            "symlink skipped"
        );
    }
    Ok(())
}
pub fn inspect(cancel: &AtomicBool) -> Result<OrphanReport, Error> {
    inspect_with_progress(cancel, &mut |_| {})
}
pub fn inspect_with_progress(
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Path),
) -> Result<OrphanReport, Error> {
    let apps = discovery::discover(&crate::config::application_roots(), cancel)?;
    scan(&apps, &crate::config::data_roots(), cancel, progress)
}
pub fn scan(
    apps: &Report<Application>,
    roots: &[PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Path),
) -> Result<OrphanReport, Error> {
    let mut report = OrphanReport { findings: vec![], issues: apps.issues.clone(), read_only: true, limitation: "An owner absent from standard application locations does not prove abandonment. Apps on other volumes, helpers and shared data may still use these paths. No findings are verified or removable; discovery and sizes may be incomplete." };
    for root in roots {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        progress(root);
        let entries = (|| -> anyhow::Result<_> {
            inspectable(root)?;
            Ok(std::fs::read_dir(root)?)
        })();
        let entries = match entries {
            Ok(entries) => entries,
            Err(e) => {
                report.issues.push(Issue {
                    path: root.clone(),
                    message: e.to_string(),
                });
                continue;
            }
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    report.issues.push(Issue {
                        path: root.clone(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();
            progress(&path);
            let evidence = (|| -> anyhow::Result<Option<(String, bool, bool)>> {
                inspectable(&path)?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if root
                    .file_name()
                    .is_some_and(|n| n == "Containers" || n == "Group Containers")
                {
                    if !entry.file_type()?.is_dir() {
                        return Ok(None);
                    }
                    let value = plist_io::read_plist(
                        &path.join(".com.apple.containermanagerd.metadata.plist"),
                    )?;
                    let dict = value
                        .as_dictionary()
                        .ok_or_else(|| anyhow::anyhow!("container metadata is not a dictionary"))?;
                    let id = dict
                        .get("MCMMetadataIdentifier")
                        .and_then(plist::Value::as_string)
                        .filter(|s| identifier(s))
                        .ok_or_else(|| anyhow::anyhow!("missing/invalid container identifier"))?;
                    let private = root.file_name().is_some_and(|n| n == "Containers")
                        && dict
                            .get("MCMMetadataContentClass")
                            .and_then(plist::Value::as_unsigned_integer)
                            == Some(2);
                    return Ok(Some((id.into(), true, !private)));
                }
                // Service state and arbitrary directory names are intentionally not inferred.
                if root.file_name().is_some_and(|n| n == "LaunchAgents") {
                    return Ok(None);
                }
                let id = name
                    .strip_suffix(".plist")
                    .or_else(|| name.strip_suffix(".savedState"))
                    .unwrap_or(&name);
                Ok(identifier(id).then(|| (id.into(), false, true)))
            })();
            let (id, metadata, shared) = match evidence {
                Ok(Some(e)) => e,
                Ok(None) => continue,
                Err(e) => {
                    report.issues.push(Issue {
                        path,
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            if id.to_ascii_lowercase().starts_with("com.apple.")
                || apps.items.iter().any(|a| related(&id, &a.bundle_id))
            {
                continue;
            }
            if report.findings.iter().any(|f| path.starts_with(&f.path)) {
                continue;
            }
            let (bytes, issues) = disk_usage::measure_with_progress(&path, cancel, progress)?;
            let size_complete = issues.is_empty();
            report.issues.extend(issues);
            report.findings.retain(|f| !f.path.starts_with(&path));
            report.findings.push(Candidate { path, bundle_id: id, bytes, size_complete, shared, classification: if metadata && !shared && apps.issues.is_empty() { Classification::Likely } else { Classification::Unknown }, evidence: if metadata { "Container MCMMetadataIdentifier identifies an owner absent from discovered applications; this does not prove exclusive use or abandonment." } else { "Filename resembles a bundle identifier absent from discovered applications; ownership and abandonment are unverified." }.into() });
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    report.findings.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn fixture() -> (tempfile::TempDir, PathBuf, Report<Application>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let apps = Report {
            items: vec![Application {
                name: "Fixture".into(),
                bundle_id: "org.fixture.app".into(),
                path: root.join("Fixture.app"),
                version: None,
                protected: false,
            }],
            issues: vec![],
        };
        (temp, root, apps)
    }
    fn run(apps: &Report<Application>, roots: &[PathBuf]) -> OrphanReport {
        scan(apps, roots, &AtomicBool::new(false), &mut |_| {}).unwrap()
    }
    fn container(root: &Path, directory: &str, id: &str, class: u64) {
        let path = root.join(directory);
        fs::create_dir_all(&path).unwrap();
        let mut dict = plist::Dictionary::new();
        dict.insert("MCMMetadataIdentifier".into(), id.into());
        dict.insert("MCMMetadataContentClass".into(), class.into());
        plist::Value::Dictionary(dict)
            .to_file_binary(path.join(".com.apple.containermanagerd.metadata.plist"))
            .unwrap();
    }
    #[test]
    fn names_are_only_unknown_and_installed_helpers_and_apple_are_suppressed() {
        let (_temp, root, apps) = fixture();
        for name in [
            "org.fixture.app.plist",
            "org.fixture.app.helper.plist",
            "com.apple.missing.plist",
            "Godot",
            "unrelated",
            "org.fixture.absent.plist",
        ] {
            fs::write(root.join(name), "data").unwrap();
        }
        let report = run(&apps, &[root]);
        assert_eq!(report.findings.len(), 1);
        let f = &report.findings[0];
        assert_eq!(f.bundle_id, "org.fixture.absent");
        assert_eq!(f.classification, Classification::Unknown);
        assert_eq!(f.bytes, 4);
        assert!(report.read_only && f.shared && f.size_complete);
        assert!(report
            .text()
            .contains("ownership and abandonment are unverified"));
    }
    #[test]
    fn metadata_candidates_are_likely_but_shared_or_incomplete_catalog_is_unknown() {
        let (_temp, root, mut apps) = fixture();
        let containers = root.join("Containers");
        container(&containers, "UUID", "org.absent.app", 2);
        container(&containers, "installed", "org.fixture.app", 2);
        let roots = [containers];
        let report = run(&apps, &roots);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].classification, Classification::Likely);
        apps.issues.push(Issue {
            path: root.clone(),
            message: "discovery denied".into(),
        });
        assert_eq!(
            run(&apps, &roots).findings[0].classification,
            Classification::Unknown
        );
        let group = root.join("Group Containers");
        container(&group, "UUID", "org.absent.group", 2);
        let report = run(&apps, &[group]);
        assert!(report.findings[0].shared);
        assert_eq!(report.findings[0].classification, Classification::Unknown);
    }
    #[test]
    fn unreadable_metadata_missing_roots_and_symlinks_are_reported() {
        let (_temp, root, apps) = fixture();
        let containers = root.join("Containers");
        fs::create_dir_all(containers.join("invalid")).unwrap();
        fs::write(
            containers.join("invalid/.com.apple.containermanagerd.metadata.plist"),
            "invalid plist",
        )
        .unwrap();
        std::os::unix::fs::symlink(&containers, root.join("linked")).unwrap();
        let report = run(
            &apps,
            &[containers, root.join("missing"), root.join("linked")],
        );
        assert!(report.findings.is_empty());
        assert_eq!(report.issues.len(), 3);
        assert!(report.text().contains("Uninspected:"));
    }
    #[test]
    fn cancellation_before_scan_and_during_measurement_is_respected() {
        let (_temp, root, apps) = fixture();
        assert!(matches!(
            scan(
                &apps,
                std::slice::from_ref(&root),
                &AtomicBool::new(true),
                &mut |_| {}
            ),
            Err(Error::Cancelled)
        ));
        let path = root.join("org.absent.app");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("data"), "data").unwrap();
        let cancel = AtomicBool::new(false);
        let result = scan(&apps, &[root], &cancel, &mut |p| {
            if p.ends_with("data") {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        // Cancellation must also be checked after traversal's final callback.
        assert!(matches!(result, Err(Error::Cancelled)));
    }
    #[test]
    fn overlapping_roots_never_double_count() {
        let (_temp, root, apps) = fixture();
        let parent = root.join("org.absent.app");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("org.absent.child"), "123").unwrap();
        let report = run(&apps, &[parent, root]);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].bytes, 3);
    }
    #[test]
    fn denied_directory_is_an_issue_not_an_empty_success() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, root, apps) = fixture();
        let denied = root.join("denied");
        fs::create_dir(&denied).unwrap();
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).unwrap();
        let inaccessible = fs::read_dir(&denied).is_err();
        let report = run(&apps, std::slice::from_ref(&denied));
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o700)).unwrap();
        if inaccessible {
            assert_eq!(report.issues.len(), 1);
        }
        assert!(report.findings.is_empty());
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["read_only"], true);
        assert!(json["issues"].is_array());
    }
}
