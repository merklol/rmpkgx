//! Append-only write-ahead journals. They describe past events, never authorize
//! future deletion. Unknown/incomplete attempts must be inspected manually.
use super::{native::open_at, trash::ItemResult};
use crate::core::application::Application;
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::File,
    io::{BufRead, BufReader, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum Event {
    Started {
        version: u32,
        #[serde(default)]
        restore: bool,
        #[serde(default)]
        data_only: bool,
        application: Application,
        paths: Vec<PathBuf>,
        remaining: usize,
        inspection_issues: usize,
    },
    Attempt {
        path: PathBuf,
    },
    Result {
        item: ItemResult,
    },
    Finished,
}
#[derive(Debug, Serialize)]
pub struct Journal {
    pub id: String,
    pub events: Vec<Event>,
    pub issues: Vec<String>,
}
impl Journal {
    pub fn application(&self) -> anyhow::Result<&Application> {
        match self.events.first() {
            Some(Event::Started {
                version: 1,
                application,
                ..
            }) => Ok(application),
            _ => anyhow::bail!("missing or unsupported journal header"),
        }
    }
    pub fn details(&self) -> String {
        let mut text = format!(
            "{}\n\nRecorded plan (historical; not a new selection):\n",
            self.text()
        );
        if let Some(Event::Started {
            paths, data_only, ..
        }) = self.events.first()
        {
            text.push_str(if *data_only {
                "Data-only cleanup\n"
            } else {
                "Application and selected data\n"
            });
            for path in paths {
                text.push_str(&format!("  {}\n", path.display()));
            }
        }
        text.push_str("\nRecorded outcomes:\n");
        for event in &self.events {
            if let Event::Result { item } = event {
                text.push_str(&format!("{:?} {}", item.status, item.source.display()));
                if let Some(path) = &item.destination {
                    text.push_str(&format!(" -> {}", path.display()));
                }
                if let Some(error) = &item.error {
                    text.push_str(&format!(" · {error}"));
                }
                text.push('\n');
            }
        }
        for path in self.unresolved_attempts() {
            text.push_str(&format!(
                "UNKNOWN {} — inspect source and Trash manually\n",
                path.display()
            ));
        }
        for issue in &self.issues {
            text.push_str(&format!("Journal error: {issue}\n"));
        }
        match super::recovery::candidates(self) {
            Ok(paths) => text.push_str(&format!("\nKnown retryable data paths: {}\n", paths.len())),
            Err(e) => text.push_str(&format!("\nRecovery unavailable: {e:#}\n")),
        }
        text.push_str("\nA fresh plan can include only recorded failed/skipped data with current scan evidence.\nMoved objects and unknown outcomes are excluded. Old approvals never carry forward.");
        text
    }
    pub fn unresolved_attempts(&self) -> Vec<PathBuf> {
        let mut pending = std::collections::BTreeSet::new();
        for event in &self.events {
            match event {
                Event::Attempt { path } => {
                    pending.insert(path.clone());
                }
                Event::Result { item } => {
                    pending.remove(&item.source);
                }
                _ => {}
            }
        }
        pending.into_iter().collect()
    }
    pub fn text(&self) -> String {
        let name = self
            .application()
            .map(|a| a.name.as_str())
            .unwrap_or("unknown");
        let finished = matches!(self.events.last(), Some(Event::Finished))
            && self.issues.is_empty()
            && self.unresolved_attempts().is_empty();
        let mut counts = [0usize; 4];
        for event in &self.events {
            if let Event::Result { item } = event {
                let index = match item.status {
                    super::trash::MoveStatus::Moved => 0,
                    super::trash::MoveStatus::MovedUnverified => 1,
                    super::trash::MoveStatus::Failed => 2,
                    super::trash::MoveStatus::Skipped => 3,
                };
                counts[index] += 1;
            }
        }
        let detail = format!(
            "moved {} · unverified {} · failed {} · skipped {} · unknown {}",
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            self.unresolved_attempts().len()
        );
        format!(
            "{} · {} · {} · {} events · {} · {} read issues · {}",
            self.id,
            if matches!(
                self.events.first(),
                Some(Event::Started { restore: true, .. })
            ) {
                "restore"
            } else {
                "cleanup"
            },
            name,
            self.events.len(),
            if finished {
                "finished (check individual results)"
            } else {
                "incomplete — inspect source paths and Trash before retrying"
            },
            self.issues.len(),
            detail
        )
    }
}
pub fn root() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("home directory unavailable")?
        .join("Library/Application Support/rmpkgx/history"))
}
fn directory(path: &Path, create: bool) -> anyhow::Result<File> {
    ensure!(path.is_absolute(), "absolute journal directory required");
    let mut current = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let name = CString::new(name.as_bytes())?;
                if create {
                    // SAFETY: valid live directory fd and NUL-terminated name.
                    let status =
                        unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) };
                    if status != 0
                        && std::io::Error::last_os_error().kind()
                            != std::io::ErrorKind::AlreadyExists
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                current = open_at(&current, &name, true)?;
            }
            _ => anyhow::bail!("journal path traversal rejected"),
        }
    }
    let metadata = current.metadata()?;
    // SAFETY: geteuid has no preconditions.
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
        "journal directory must be owned by current user and private (0700)"
    );
    Ok(current)
}
fn file_name(id: &str) -> anyhow::Result<CString> {
    ensure!(
        !id.is_empty() && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_digit() || b == b'-'),
        "invalid journal ID"
    );
    Ok(CString::new(format!("{id}.jsonl"))?)
}
pub struct Writer {
    pub id: String,
    file: File,
}
impl Writer {
    pub fn start(
        application: &Application,
        paths: &[PathBuf],
        remaining: usize,
        issues: usize,
        data_only: bool,
    ) -> anyhow::Result<Self> {
        Self::start_at_kind(&root()?, application, paths, remaining, issues, data_only)
    }
    #[cfg(test)]
    fn start_at(
        root: &Path,
        application: &Application,
        paths: &[PathBuf],
        remaining: usize,
        issues: usize,
    ) -> anyhow::Result<Self> {
        Self::start_at_kind(root, application, paths, remaining, issues, false)
    }
    fn start_at_kind(
        root: &Path,
        application: &Application,
        paths: &[PathBuf],
        remaining: usize,
        issues: usize,
        data_only: bool,
    ) -> anyhow::Result<Self> {
        Self::start_at_operation(
            root,
            application,
            paths,
            remaining,
            issues,
            data_only,
            false,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn start_at_operation(
        root: &Path,
        application: &Application,
        paths: &[PathBuf],
        remaining: usize,
        issues: usize,
        data_only: bool,
        restore: bool,
    ) -> anyhow::Result<Self> {
        let directory = directory(root, true)?;
        let id = format!(
            "{}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            std::process::id()
        );
        let name = file_name(&id)?;
        // SAFETY: live directory fd, valid name; exclusive creation never overwrites.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a new owned descriptor.
        let mut writer = Self {
            id,
            file: unsafe { File::from_raw_fd(fd) },
        };
        writer.append(&Event::Started {
            version: 1,
            restore,
            data_only,
            application: application.clone(),
            paths: paths.to_vec(),
            remaining,
            inspection_issues: issues,
        })?;
        directory.sync_all()?;
        Ok(writer)
    }
    pub fn start_restore(application: &Application, paths: &[PathBuf]) -> anyhow::Result<Self> {
        Self::start_at_operation(&root()?, application, paths, 0, 0, false, true)
    }
    pub fn append(&mut self, event: &Event) -> anyhow::Result<()> {
        let mut bytes = serde_json::to_vec(event)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        Ok(())
    }
}
pub fn read(id: &str) -> anyhow::Result<Journal> {
    read_at(&root()?, id)
}
fn read_at(root: &Path, id: &str) -> anyhow::Result<Journal> {
    let directory = directory(root, false)?;
    let file = open_at(&directory, &file_name(id)?, false)?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0,
        "unsafe journal file"
    );
    ensure!(
        metadata.len() <= 16 * 1024 * 1024,
        "journal exceeds 16 MiB inspection limit"
    );
    let mut report = Journal {
        id: id.into(),
        events: vec![],
        issues: vec![],
    };
    for (index, line) in BufReader::new(file).split(b'\n').enumerate() {
        match line {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(event) => report.events.push(event),
                Err(e) => {
                    report.issues.push(format!(
                        "line {}: {e}; later events are untrusted",
                        index + 1
                    ));
                    break;
                }
            },
            Err(e) => {
                report.issues.push(e.to_string());
                break;
            }
        }
    }
    report.application()?;
    Ok(report)
}
pub fn list() -> anyhow::Result<Vec<Journal>> {
    let root = root()?;
    if !root.try_exists()? {
        return Ok(vec![]);
    }
    let _directory = directory(&root, false)?;
    let mut journals = vec![];
    for entry in std::fs::read_dir(&root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "jsonl") {
            let id = path
                .file_stem()
                .context("missing journal name")?
                .to_str()
                .context("invalid journal filename")?;
            journals.push(match read_at(&root, id) {
                Ok(journal) => journal,
                Err(e) => Journal {
                    id: id.to_owned(),
                    events: vec![],
                    issues: vec![format!("{e:#}")],
                },
            });
        }
    }
    journals.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(journals)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> Application {
        Application {
            name: "Fixture".into(),
            bundle_id: "org.fixture".into(),
            path: "/fixture/app".into(),
            version: None,
            protected: false,
        }
    }
    #[test]
    fn journal_round_trip_and_interrupted_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("history");
        let mut writer = Writer::start_at(&root, &app(), &["/fixture/data".into()], 1, 2).unwrap();
        writer
            .append(&Event::Attempt {
                path: "/fixture/data".into(),
            })
            .unwrap();
        let report = read_at(&root, &writer.id).unwrap();
        assert_eq!(report.events.len(), 2);
        assert!(report.text().contains("incomplete"));
        assert_eq!(
            report.unresolved_attempts(),
            vec![PathBuf::from("/fixture/data")]
        );
        writer
            .append(&Event::Result {
                item: ItemResult {
                    identity: None,
                    source: "/fixture/data".into(),
                    status: super::super::trash::MoveStatus::Skipped,
                    destination: None,
                    error: Some("fixture".into()),
                },
            })
            .unwrap();
        writer.append(&Event::Finished).unwrap();
        assert!(read_at(&root, &writer.id)
            .unwrap()
            .text()
            .contains("finished"));
        writer.file.write_all(b"{broken").unwrap();
        writer.file.sync_all().unwrap();
        let report = read_at(&root, &writer.id).unwrap();
        assert_eq!(report.issues.len(), 1);
        assert!(report.text().contains("incomplete"));
    }
    #[test]
    fn traversal_and_symlink_journals_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        assert!(file_name("../1").is_err());
        std::os::unix::fs::symlink(&root, root.join("alias")).unwrap();
        assert!(directory(&root.join("alias"), false).is_err());
        let journal_root = root.join("history");
        let writer = Writer::start_at(&journal_root, &app(), &[], 0, 0).unwrap();
        std::os::unix::fs::symlink(
            journal_root.join(format!("{}.jsonl", writer.id)),
            journal_root.join("123.jsonl"),
        )
        .unwrap();
        assert!(read_at(&journal_root, "123").is_err());
    }
    #[test]
    fn write_failures_and_nonprivate_directories_are_not_silently_accepted() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("readonly");
        std::fs::write(&data, "fixture").unwrap();
        let mut writer = Writer {
            id: "1".into(),
            file: File::open(&data).unwrap(),
        };
        assert!(writer.append(&Event::Finished).is_err());
        let unsafe_root = root.join("shared");
        std::fs::create_dir(&unsafe_root).unwrap();
        std::fs::set_permissions(&unsafe_root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Writer::start_at(&unsafe_root, &app(), &[], 0, 0).is_err());
        assert_eq!(std::fs::read_dir(&unsafe_root).unwrap().count(), 0);
    }
}
