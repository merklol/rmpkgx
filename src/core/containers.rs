//! Observed macOS ContainerManager plist schema, not a stable public API.
//! Never treats group containers or unknown classes as exclusive app data.
use super::{
    application::Application,
    association::{associate, Confidence},
};
use anyhow::{ensure, Context};
use std::path::Path;
const METADATA: &str = ".com.apple.containermanagerd.metadata.plist";
#[cfg(test)]
const MAX_BYTES: u64 = 1024 * 1024;
#[derive(Debug, Clone)]
pub struct Evidence {
    pub confidence: Confidence,
    pub reason: String,
    pub shared: bool,
    pub issue: Option<String>,
}
fn read_metadata(container: &Path) -> anyhow::Result<plist::Value> {
    super::plist_io::read_plist(&container.join(METADATA))
}
pub fn evaluate(app: &Application, path: &Path, group: bool) -> anyhow::Result<Option<Evidence>> {
    let name = path
        .file_name()
        .context("container has no name")?
        .to_string_lossy();
    let filename_match = associate(app, &name);
    let metadata = match read_metadata(path) {
        Ok(value) => value,
        Err(error) => {
            // Missing metadata permits manual review of a name match, but never
            // silently promotes it or hides an unreadable unknown container.
            if let Some((_, reason)) = filename_match {
                return Ok(Some(Evidence { confidence: Confidence::Medium, reason: format!("{reason}; container metadata unavailable: {error:#}; manual ownership review required"), shared: group, issue: Some(format!("{error:#}")) }));
            }
            return Err(error.context(format!(
                "container ownership could not be inspected: {}",
                path.display()
            )));
        }
    };
    let dict = metadata
        .as_dictionary()
        .context("container metadata is not a dictionary")?;
    let identifier = dict
        .get("MCMMetadataIdentifier")
        .and_then(plist::Value::as_string)
        .filter(|id| !id.is_empty())
        .context("missing or invalid MCMMetadataIdentifier")?;
    if identifier != app.bundle_id {
        ensure!(
            filename_match.is_none(),
            "container metadata identifies another owner; filename association rejected"
        );
        return Ok(None);
    }
    let class = dict
        .get("MCMMetadataContentClass")
        .and_then(plist::Value::as_unsigned_integer);
    let private = !group && class == Some(2);
    Ok(Some(Evidence {
        confidence: if private { Confidence::High } else { Confidence::Medium },
        reason: format!("{METADATA}: MCMMetadataIdentifier exactly matches bundle ID {}; MCMMetadataContentClass={}; {}", app.bundle_id, class.map_or_else(|| "unknown".into(), |c| c.to_string()), if private { "app data container metadata (not proof of exclusive use by one installation)" } else { "shared or unknown container class; review only" }),
        shared: !private,
        issue: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> Application {
        Application {
            name: "Fixture".into(),
            bundle_id: "org.fixture.app".into(),
            path: "/fixture/app".into(),
            version: None,
            protected: false,
        }
    }
    fn metadata(path: &Path, identifier: &str, class: u64, binary: bool) {
        let mut dict = plist::Dictionary::new();
        dict.insert("MCMMetadataIdentifier".into(), identifier.into());
        dict.insert("MCMMetadataContentClass".into(), class.into());
        let value = plist::Value::Dictionary(dict);
        if binary {
            value.to_file_binary(path.join(METADATA)).unwrap();
        } else {
            value.to_file_xml(path.join(METADATA)).unwrap();
        }
    }
    #[test]
    fn exact_metadata_finds_uuid_named_containers_in_xml_and_binary() {
        for binary in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().canonicalize().unwrap().join("UUID");
            std::fs::create_dir(&path).unwrap();
            metadata(&path, "org.fixture.app", 2, binary);
            let found = evaluate(&app(), &path, false).unwrap().unwrap();
            assert_eq!(found.confidence, Confidence::High);
            assert!(!found.shared);
            assert!(found.reason.contains("MCMMetadataIdentifier"));
        }
    }
    #[test]
    fn conflicting_metadata_overrides_filename_similarity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("org.fixture.app");
        std::fs::create_dir(&path).unwrap();
        metadata(&path, "org.other.app", 2, false);
        assert!(evaluate(&app(), &path, false).is_err());
        let unknown = temp.path().canonicalize().unwrap().join("unrelated");
        std::fs::create_dir(&unknown).unwrap();
        metadata(&unknown, "org.other.app", 2, false);
        assert!(evaluate(&app(), &unknown, false).unwrap().is_none());
    }
    #[test]
    fn group_and_unknown_classes_are_never_private_high_confidence() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        metadata(&path, "org.fixture.app", 2, false);
        let group = evaluate(&app(), &path, true).unwrap().unwrap();
        assert!(group.shared);
        assert_eq!(group.confidence, Confidence::Medium);
        metadata(&path, "org.fixture.app", 99, false);
        let unknown = evaluate(&app(), &path, false).unwrap().unwrap();
        assert!(unknown.shared);
        assert_eq!(unknown.confidence, Confidence::Medium);
    }
    #[test]
    fn missing_malformed_and_oversized_metadata_never_auto_promote_names() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("org.fixture.app");
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            evaluate(&app(), &path, false).unwrap().unwrap().confidence,
            Confidence::Medium
        );
        // plist accepts a bare string as an OpenStep plist, but this is not
        // a container metadata dictionary and must be rejected.
        std::fs::write(path.join(METADATA), "malformed").unwrap();
        assert!(evaluate(&app(), &path, false).is_err());
        std::fs::write(path.join(METADATA), vec![0u8; MAX_BYTES as usize + 1]).unwrap();
        assert_eq!(
            evaluate(&app(), &path, false).unwrap().unwrap().confidence,
            Confidence::Medium
        );
    }
    #[test]
    fn symlink_metadata_and_ancestors_are_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let real = root.join("real");
        let container = root.join("org.fixture.app");
        std::fs::create_dir(&real).unwrap();
        std::fs::create_dir(&container).unwrap();
        metadata(&real, "org.fixture.app", 2, false);
        std::os::unix::fs::symlink(real.join(METADATA), container.join(METADATA)).unwrap();
        assert_eq!(
            evaluate(&app(), &container, false)
                .unwrap()
                .unwrap()
                .confidence,
            Confidence::Medium
        );
        std::os::unix::fs::symlink(&real, root.join("alias")).unwrap();
        assert!(read_metadata(&root.join("alias")).is_err());
        assert!(read_metadata(&root.join("../real")).is_err());
    }
}
