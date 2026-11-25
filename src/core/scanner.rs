use super::{
    application::Application,
    association::{for_path, Confidence},
    disk_usage, Issue, Report,
};
use crate::{cleanup::safety, error::Error};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub review_only: bool,
    pub path: PathBuf,
    pub bytes: u64,
    pub reason: String,
    pub confidence: Confidence,
    pub safety: String,
    pub shared: bool,
    pub selected: bool,
}
pub fn scan(
    app: &Application,
    roots: &[PathBuf],
    cancel: &AtomicBool,
) -> Result<Report<Finding>, Error> {
    scan_with_progress(app, roots, cancel, &mut |_| {})
}

pub fn scan_with_progress(
    app: &Application,
    roots: &[PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&std::path::Path),
) -> Result<Report<Finding>, Error> {
    let mut report = Report {
        items: vec![],
        issues: vec![],
    };
    for root in roots {
        progress(root);
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // Reject symlinked ancestors, not just the final directory.
        if let Err(e) = safety::validate(root) {
            report.issues.push(Issue {
                path: root.clone(),
                message: e.to_string(),
            });
            continue;
        }
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
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
            // Metadata is only meaningful for actual container directories.
            let container_root = root
                .file_name()
                .is_some_and(|name| name == "Containers" || name == "Group Containers");
            if container_root {
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => {}
                    Ok(kind) if kind.is_symlink() => {
                        report.issues.push(Issue {
                            path,
                            message: "symlink container skipped".into(),
                        });
                        continue;
                    }
                    Ok(_) => continue,
                    Err(error) => {
                        report.issues.push(Issue {
                            path,
                            message: error.to_string(),
                        });
                        continue;
                    }
                }
            }
            let evidence = match for_path(app, &path) {
                Ok(Some(evidence)) => evidence,
                Ok(None) => continue,
                Err(error) => {
                    report.issues.push(Issue {
                        path,
                        message: format!("{error:#}"),
                    });
                    continue;
                }
            };
            if let Some(issue) = &evidence.issue {
                report.issues.push(Issue {
                    path: if container_root {
                        path.join(".com.apple.containermanagerd.metadata.plist")
                    } else {
                        path.clone()
                    },
                    message: issue.clone(),
                });
            }
            let confidence = evidence.confidence;
            let reason = evidence.reason;
            if let Err(e) = safety::validate(&path) {
                report.issues.push(Issue {
                    path,
                    message: e.to_string(),
                });
                continue;
            }
            if report
                .items
                .iter()
                .any(|f: &Finding| path.starts_with(&f.path))
            {
                continue;
            }
            let (bytes, issues) = disk_usage::measure_with_progress(&path, cancel, progress)?;
            let incomplete = !issues.is_empty();
            report.issues.extend(issues);
            let shared = evidence.shared;
            let review_only = root.file_name().is_some_and(|name| name == "LaunchAgents");
            report.items.push(Finding {
                review_only,
                path,
                bytes,
                reason,
                confidence,
                shared,
                safety: if review_only {
                    "LaunchAgent: inspect only; loaded state unknown"
                } else if shared {
                    "Shared: review only"
                } else {
                    "Review required"
                }
                .into(),
                selected: confidence == Confidence::High
                    && !shared
                    && !review_only
                    && !incomplete
                    && !app.protected,
            });
        }
    }
    Ok(report)
}
