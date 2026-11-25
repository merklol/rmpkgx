//! Descriptor-relative, no-clobber move primitive. Not a production Trash API:
//! Darwin has no compare-inode-and-rename operation. A final-entry replacement
//! can still be moved between our last check and rename. Keep this test-only
//! until this residual risk has an explicitly reviewed policy.
use anyhow::{ensure, Context};
use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub birth: i64,
    pub birth_ns: i64,
}
pub struct Pinned {
    path: PathBuf,
    parent: File,
    name: CString,
    object: File,
    fingerprint: (u64, i64, i64, i64, i64),
}
pub(super) fn open_at(parent: &File, name: &CString, directory: bool) -> anyhow::Result<File> {
    let flags = libc::O_RDONLY
        | libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent is a live fd and name is a NUL-terminated string.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful openat returns a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn directory(path: &Path) -> anyhow::Result<File> {
    ensure!(path.is_absolute(), "absolute path required");
    let mut fd = File::open("/")?;
    for c in path.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(name) => fd = open_at(&fd, &CString::new(name.as_bytes())?, true)?,
            _ => anyhow::bail!("path traversal rejected"),
        }
    }
    Ok(fd)
}
fn same(a: &File, b: &File) -> anyhow::Result<bool> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}
impl Pinned {
    pub fn capture(path: &Path) -> anyhow::Result<Self> {
        let parent = directory(path.parent().context("missing parent")?)?;
        let name = CString::new(path.file_name().context("missing filename")?.as_bytes())?;
        let object = open_at(&parent, &name, false)?;
        let metadata = object.metadata()?;
        ensure!(
            metadata.is_file() || metadata.is_dir(),
            "special files are not supported"
        );
        let fingerprint = (
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        );
        Ok(Self {
            fingerprint,
            path: path.into(),
            parent,
            name,
            object,
        })
    }
    #[cfg(target_os = "macos")]
    pub fn matches(&self, path: &Path) -> anyhow::Result<()> {
        let current = Self::capture(path)?;
        ensure!(
            same(&current.object, &self.object)?,
            "file reference does not identify the reviewed object"
        );
        Ok(())
    }
    pub fn identity(&self) -> anyhow::Result<FileIdentity> {
        let m = self.object.metadata()?;
        let birth = m.created()?.duration_since(std::time::UNIX_EPOCH)?;
        Ok(FileIdentity {
            device: m.dev(),
            inode: m.ino(),
            birth: birth.as_secs().try_into()?,
            birth_ns: birth.subsec_nanos().into(),
        })
    }
    pub fn revalidate_identity(&self) -> anyhow::Result<()> {
        let current = Self::capture(&self.path)?;
        ensure!(
            same(&current.parent, &self.parent)? && same(&current.object, &self.object)?,
            "reviewed path or parent replaced"
        );
        Ok(())
    }
    pub fn revalidate(&self) -> anyhow::Result<()> {
        let parent = directory(self.path.parent().context("missing parent")?)?;
        ensure!(
            same(&parent, &self.parent)?,
            "parent directory was replaced"
        );
        let current = open_at(&parent, &self.name, false)?;
        ensure!(
            same(&current, &self.object)?,
            "reviewed object was replaced"
        );
        let m = current.metadata()?;
        ensure!(
            self.fingerprint
                == (
                    m.len(),
                    m.mtime(),
                    m.mtime_nsec(),
                    m.ctime(),
                    m.ctime_nsec()
                ),
            "reviewed object changed; rescan required"
        );
        Ok(())
    }
    #[cfg(all(test, target_os = "macos"))]
    fn move_to(&self, destination: &Path) -> anyhow::Result<()> {
        self.move_with_hook(destination, || {})
    }
    #[cfg(all(test, target_os = "macos"))]
    fn move_with_hook(&self, destination: &Path, after_check: impl FnOnce()) -> anyhow::Result<()> {
        let parent = directory(destination.parent().context("missing destination parent")?)?;
        let name = CString::new(
            destination
                .file_name()
                .context("missing destination name")?
                .as_bytes(),
        )?;
        self.revalidate()?;
        after_check();
        // SAFETY: both descriptors and strings are valid throughout the call.
        // RENAME_EXCL atomically refuses overwrites. Cross-device errors propagate.
        let result = unsafe {
            libc::renameatx_np(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let moved = open_at(&parent, &name, false)?;
        ensure!(
            same(&moved, &self.object)?,
            "race detected after move; inspect destination manually"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_and_symlinks_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("data");
        std::fs::write(&path, "old").unwrap();
        let pin = Pinned::capture(&path).unwrap();
        std::fs::rename(&path, root.join("original")).unwrap();
        std::fs::write(&path, "new").unwrap();
        assert!(pin.revalidate().is_err());
        std::os::unix::fs::symlink(&path, root.join("alias")).unwrap();
        assert!(Pinned::capture(&root.join("alias")).is_err());
        std::os::unix::fs::symlink(&root, root.join("parent-link")).unwrap();
        assert!(Pinned::capture(&root.join("parent-link/data")).is_err());
        assert!(Pinned::capture(&root.join("../data")).is_err());
    }
    #[test]
    fn in_place_changes_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("data");
        std::fs::write(&path, "old").unwrap();
        let pin = Pinned::capture(&path).unwrap();
        std::fs::write(&path, "new content").unwrap();
        assert!(pin.revalidate().is_err());
    }
    #[test]
    fn replaced_parent_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("parent")).unwrap();
        std::fs::write(root.join("parent/data"), "old").unwrap();
        let pin = Pinned::capture(&root.join("parent/data")).unwrap();
        std::fs::rename(root.join("parent"), root.join("renamed")).unwrap();
        std::fs::create_dir(root.join("parent")).unwrap();
        std::fs::write(root.join("parent/data"), "new").unwrap();
        assert!(pin.revalidate().is_err());
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn fixture_move_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("source");
        let destination = root.join("destination");
        std::fs::write(&path, "reviewed").unwrap();
        let pin = Pinned::capture(&path).unwrap();
        std::fs::write(&destination, "existing").unwrap();
        assert!(pin.move_to(&destination).is_err());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "existing");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "reviewed");
        pin.move_to(&root.join("unique")).unwrap();
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(root.join("unique")).unwrap(),
            "reviewed"
        );
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn final_entry_race_is_detected_but_not_prevented() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let destination = root.join("candidate-trash");
        std::fs::write(&source, "approved").unwrap();
        let pin = Pinned::capture(&source).unwrap();
        let result = pin.move_with_hook(&destination, || {
            std::fs::rename(&source, root.join("original")).unwrap();
            std::fs::write(&source, "unapproved replacement").unwrap();
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("original")).unwrap(),
            "approved"
        );
        // Demonstrates why this adapter must remain test-only: detection occurs
        // after an unapproved object has already moved, even with pinned parents.
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "unapproved replacement"
        );
    }
}
