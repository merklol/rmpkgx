//! Restore only verified journaled objects. No overwrite, parent creation,
//! symlink traversal, cross-volume copy/delete, or path-only identity fallback.
use super::{
    history::{Event, Journal, Writer},
    native::Pinned,
    reference::Reference,
    trash::{ItemResult, MoveStatus},
};
use crate::core::application::Application;
use anyhow::{ensure, Context};
use std::path::{Path, PathBuf};

pub struct Entry {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub selected: bool,
    pub blocked: Option<String>,
    pub access_denied: bool,
    prepared: Option<Prepared>,
}
struct Prepared {
    pin: Pinned,
    reference: Reference,
    parent: Pinned,
    parent_ref: Reference,
}
pub struct RestorePlan {
    pub journal_id: String,
    pub entries: Vec<Entry>,
    application: Application,
}
fn recorded_items(journal: &Journal) -> anyhow::Result<Vec<&ItemResult>> {
    ensure!(
        journal.issues.is_empty(),
        "damaged journal cannot authorize restoration"
    );
    ensure!(
        !matches!(
            journal.events.first(),
            Some(Event::Started { restore: true, .. })
        ),
        "this journal records restoration, not objects moved to Trash"
    );
    let approved = match journal.events.first() {
        Some(Event::Started { paths, .. }) => paths,
        _ => anyhow::bail!("missing header"),
    };
    let unresolved = journal.unresolved_attempts();
    let mut items = std::collections::BTreeMap::new();
    for event in &journal.events {
        if let Event::Result { item } = event {
            items.insert(item.source.clone(), item);
        }
    }
    Ok(items
        .into_values()
        .filter(|item| {
            item.status == MoveStatus::Moved
                && approved.contains(&item.source)
                && !unresolved.contains(&item.source)
        })
        .collect())
}
impl RestorePlan {
    pub fn prepare(journal: &Journal) -> anyhow::Result<Self> {
        let app = journal.application()?.clone();
        let trash = dirs::home_dir().context("home unavailable")?.join(".Trash");
        let entries = recorded_items(journal)?
            .into_iter()
            .map(|item| {
                let source = item.destination.clone().unwrap_or_default();
                let destination = item.source.clone();
                let prepared = (|| {
                    let identity = item.identity.as_ref().context(
                        "old journal has no saved object identity; restore through Finder",
                    )?;
                    ensure!(
                        source.parent() == Some(trash.as_path()),
                        "only recorded objects directly in the user's Trash are supported"
                    );
                    ensure!(
                        !super::safety::protected(&destination),
                        "protected restore destination"
                    );
                    let app_target = destination == app.path
                        && crate::config::application_roots()
                            .iter()
                            .any(|root| destination.starts_with(root) && destination != *root);
                    let data_target = crate::config::data_roots()
                        .iter()
                        .any(|root| destination.parent() == Some(root.as_path()));
                    ensure!(
                        app_target || data_target,
                        "restore target is outside supported application/data roots"
                    );
                    let parent_path = destination.parent().context("missing destination parent")?;
                    super::safety::validate(parent_path)?;
                    ensure_absent(&destination)?;
                    let pin = Pinned::capture(&source)
                        .context("cannot inspect the recorded Trash object")?;
                    ensure!(
                        pin.identity()? == *identity,
                        "Trash object identity differs from the journal; restore blocked"
                    );
                    if app_target {
                        ensure!(
                            Application::read(&source)?.bundle_id == app.bundle_id,
                            "bundle metadata differs from the journal"
                        );
                    }
                    let reference = Reference::capture(&source, &pin)
                        .context("cannot reference the recorded Trash object")?;
                    let parent = Pinned::capture(parent_path)
                        .context("cannot inspect the original destination parent")?;
                    let parent_ref = Reference::capture(parent_path, &parent)?;
                    Ok(Prepared {
                        pin,
                        reference,
                        parent,
                        parent_ref,
                    })
                })();
                let (prepared, blocked, access_denied) = match prepared {
                    Ok(p) => (Some(p), None, false),
                    Err(e) => (None, Some(blocked_reason(&e)), is_access_denied(&e)),
                };
                Entry {
                    source,
                    destination,
                    selected: false,
                    prepared,
                    blocked,
                    access_denied,
                }
            })
            .collect();
        Ok(Self {
            journal_id: journal.id.clone(),
            entries,
            application: app,
        })
    }
    pub fn toggle(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            if entry.blocked.is_none() {
                entry.selected = !entry.selected;
            }
        }
    }
    pub fn preview(&self) -> anyhow::Result<String> {
        ensure!(
            self.entries.iter().any(|e| e.selected),
            "select at least one restorable object with Space"
        );
        let mut text = "Restore selected objects from Trash?\nNothing has moved yet. Existing destinations will never be overwritten.\n\n".to_owned();
        for entry in self.entries.iter().filter(|e| e.selected) {
            text.push_str(&format!(
                "{}\n  -> {}\n",
                entry.source.display(),
                entry.destination.display()
            ));
        }
        Ok(text)
    }
    pub fn execute(self) -> anyhow::Result<String> {
        self.preview()?;
        crate::platform::macos::ensure_stopped(&self.application.path)?;
        let selected: Vec<_> = self.entries.iter().filter(|e| e.selected).collect();
        // Preflight the entire selection before creating intents or moving anything.
        for entry in &selected {
            ensure_absent(&entry.destination)?;
            let prepared = entry.prepared.as_ref().context("blocked restore entry")?;
            prepared.pin.revalidate()?;
            prepared.parent.revalidate_identity()?;
        }
        let paths = selected
            .iter()
            .map(|e| e.source.clone())
            .collect::<Vec<_>>();
        let mut journal = Writer::start_restore(&self.application, &paths)?;
        let mut text = String::new();
        let mut stopped = false;
        let mut journal_error = false;
        for entry in selected {
            if stopped {
                text.push_str(&format!("SKIPPED {}\n", entry.source.display()));
                if !journal_error {
                    let item = ItemResult {
                        identity: None,
                        source: entry.source.clone(),
                        status: MoveStatus::Skipped,
                        destination: None,
                        error: Some("skipped after earlier restore failure".into()),
                    };
                    if let Err(error) = journal.append(&Event::Result { item }) {
                        text.push_str(&format!("JOURNAL ERROR: {error:#}\n"));
                        journal_error = true;
                    }
                }
                continue;
            }
            let prepared = entry.prepared.as_ref().context("blocked entry")?;
            if let Err(error) = journal.append(&Event::Attempt {
                path: entry.source.clone(),
            }) {
                text.push_str(&format!("JOURNAL ERROR before restore: {error:#}\n"));
                stopped = true;
                journal_error = true;
                continue;
            }
            let result =
                crate::platform::macos::ensure_stopped(&self.application.path).and_then(|()| {
                    prepared.reference.restore_to(
                        &prepared.pin,
                        &prepared.parent_ref,
                        &prepared.parent,
                        &entry.destination,
                    )
                });
            let item = match result {
                Ok(receipt) => ItemResult {
                    identity: prepared.pin.identity().ok(),
                    source: entry.source.clone(),
                    status: receipt.status,
                    destination: receipt.destination,
                    error: receipt.warning,
                },
                Err(error) => ItemResult {
                    identity: None,
                    source: entry.source.clone(),
                    status: MoveStatus::Failed,
                    destination: None,
                    error: Some(format!("{error:#}")),
                },
            };
            text.push_str(&format!(
                "{} {} -> {}{}\n",
                if item.status == MoveStatus::Moved {
                    "RESTORED"
                } else if item.status == MoveStatus::MovedUnverified {
                    "RESTORED — VERIFICATION UNAVAILABLE"
                } else {
                    "FAILED"
                },
                entry.source.display(),
                entry.destination.display(),
                item.error
                    .as_ref()
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            ));
            stopped = item.status != MoveStatus::Moved;
            if let Err(error) = journal.append(&Event::Result { item }) {
                text.push_str(&format!(
                    "JOURNAL ERROR after operation: {error:#}; inspect both locations\n"
                ));
                stopped = true;
                journal_error = true;
            }
        }
        if !journal_error {
            if let Err(error) = journal.append(&Event::Finished) {
                text.push_str(&format!("JOURNAL ERROR: {error:#}\n"));
            }
        }
        text.push_str(&format!("\nRestore journal: {}\nNo objects were overwritten. Refresh history before another operation.", journal.id));
        Ok(text)
    }
}
fn is_access_denied(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|error| {
            error.kind() == std::io::ErrorKind::PermissionDenied
                || matches!(error.raw_os_error(), Some(libc::EPERM | libc::EACCES))
        })
}
fn blocked_reason(error: &anyhow::Error) -> String {
    let denied = is_access_denied(error);
    if denied {
        format!("RESTORE BLOCKED — access denied: {error:#}\nCheck the hosting terminal/app in Privacy & Security > Full Disk Access; restart it after changes.\nThis is not a guaranteed fix. If still blocked, restore through Finder; no permissions were changed.")
    } else {
        format!("{error:#}")
    }
}
fn ensure_absent(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => anyhow::bail!("destination exists or cannot be inspected; restore blocked"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restoration_never_overwrites_a_file_or_even_a_dangling_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("original");
        assert!(ensure_absent(&destination).is_ok());
        std::os::unix::fs::symlink(temp.path().join("missing"), &destination).unwrap();
        assert!(ensure_absent(&destination).is_err());
    }
    #[test]
    fn old_journal_and_unverified_outcomes_are_never_silently_accepted() {
        let app = Application {
            name: "fixture".into(),
            path: "/Applications/fixture.app".into(),
            bundle_id: "org.fixture".into(),
            version: None,
            protected: false,
        };
        let mut journal = Journal {
            id: "123".into(),
            issues: vec![],
            events: vec![
                Event::Started {
                    version: 1,
                    restore: false,
                    data_only: false,
                    application: app,
                    paths: vec!["/Applications/fixture.app".into()],
                    remaining: 0,
                    inspection_issues: 0,
                },
                Event::Result {
                    item: ItemResult {
                        identity: None,
                        source: "/Applications/fixture.app".into(),
                        destination: Some("/fixture/trash/app".into()),
                        status: MoveStatus::MovedUnverified,
                        error: None,
                    },
                },
            ],
        };
        assert!(recorded_items(&journal).unwrap().is_empty());
        if let Event::Result { item } = &mut journal.events[1] {
            item.status = MoveStatus::Moved;
        }
        let plan = RestorePlan::prepare(&journal).unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert!(plan.entries[0]
            .blocked
            .as_ref()
            .unwrap()
            .contains("no saved object identity"));
        assert!(!plan.entries[0].selected);
        assert!(plan.preview().is_err());
        journal.issues.push("truncated".into());
        assert!(RestorePlan::prepare(&journal).is_err());
    }
    #[test]
    fn trash_privacy_denial_is_explained_without_hiding_the_original_error() {
        let error = anyhow::Error::from(std::io::Error::from_raw_os_error(libc::EPERM))
            .context("cannot inspect the recorded Trash object");
        let reason = blocked_reason(&error);
        assert!(reason.contains("access denied"));
        assert!(reason.contains("os error 1"));
        assert!(reason.contains("hosting terminal/app"));
        assert!(reason.contains("not a guaranteed fix"));
        assert!(reason.contains("Finder"));
        assert_eq!(
            blocked_reason(&anyhow::anyhow!("identity mismatch")),
            "identity mismatch"
        );
    }
}
