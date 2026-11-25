//! Bounded plist reading shared by container and LaunchAgent inspection.
use anyhow::{ensure, Context};
use std::{
    ffi::CString,
    fs::File,
    io::{Cursor, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path},
};
fn open_at(parent: &File, name: &std::ffi::OsStr, directory: bool) -> anyhow::Result<File> {
    let name = CString::new(name.as_bytes())?;
    // SAFETY: live directory descriptor and NUL-terminated component; every
    // component is independently opened without following symlinks.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | if directory { libc::O_DIRECTORY } else { 0 },
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful openat returns a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub(super) fn regular_file(path: &Path) -> anyhow::Result<File> {
    ensure!(path.is_absolute(), "absolute path required");
    let mut directory = File::open("/")?;
    let parent = path.parent().context("missing parent")?;
    for component in parent.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => directory = open_at(&directory, name, true)?,
            _ => anyhow::bail!("path traversal rejected"),
        }
    }
    ensure!(
        !path.components().any(|c| matches!(c, Component::ParentDir)),
        "path traversal rejected"
    );
    let file = open_at(
        &directory,
        path.file_name().context("missing filename")?,
        false,
    )?;
    ensure!(file.metadata()?.is_file(), "not a regular file");
    Ok(file)
}
pub(super) fn read_plist(path: &Path) -> anyhow::Result<plist::Value> {
    const MAX_BYTES: u64 = 1024 * 1024;
    let file = regular_file(path)?;
    ensure!(file.metadata()?.len() <= MAX_BYTES, "plist exceeds 1 MiB");
    let mut bytes = vec![];
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_BYTES, "plist exceeds 1 MiB");
    plist::Value::from_reader(Cursor::new(bytes)).context("malformed plist")
}
