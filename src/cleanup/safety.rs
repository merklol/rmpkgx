use crate::error::Error;
use std::path::{Component, Path};
pub fn protected(path: &Path) -> bool {
    [
        "/System", "/Library", "/usr", "/bin", "/sbin", "/private", "/dev",
    ]
    .iter()
    .any(|p| path.starts_with(p))
        || path == Path::new("/")
}
/// Reject traversal and every symlink component. This is a preview check, not a race-free deletion primitive.
pub fn validate(path: &Path) -> Result<(), Error> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || protected(path)
    {
        return Err(Error::Unsafe(path.display().to_string()));
    }
    let mut prefix = std::path::PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        match std::fs::symlink_metadata(&prefix) {
            Ok(m) if !m.file_type().is_symlink() => {}
            _ => return Err(Error::Unsafe(prefix.display().to_string())),
        }
    }
    Ok(())
}
