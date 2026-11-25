use super::Issue;
use crate::error::Error;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Logical bytes, not allocated blocks. Symlink targets are never measured.
pub fn measure(path: &Path, cancel: &AtomicBool) -> Result<(u64, Vec<Issue>), Error> {
    measure_with_progress(path, cancel, &mut |_| {})
}

pub fn measure_with_progress(
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Path),
) -> Result<(u64, Vec<Issue>), Error> {
    let mut bytes = 0u64;
    let mut issues = vec![];
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if let Ok(ref entry) = entry {
            progress(entry.path());
        }
        match entry {
            Ok(e) if e.file_type().is_file() => match e.metadata() {
                Ok(m) => bytes = bytes.saturating_add(m.len()),
                Err(e) => issues.push(Issue {
                    path: path.into(),
                    message: e.to_string(),
                }),
            },
            Ok(_) => {}
            Err(e) => issues.push(Issue {
                path: e.path().unwrap_or(path).into(),
                message: e.to_string(),
            }),
        }
    }
    Ok((bytes, issues))
}
