//! Immutable review bound to pinned filesystem identities and native object references.
use super::{native::Pinned, plan::Plan};
use crate::{config, core::application::Application};
use anyhow::Context;
use std::path::PathBuf;

pub struct Review {
    paths: Vec<PathBuf>,
    data_only: bool,
    data_findings: Vec<crate::core::scanner::Finding>,
    remaining: usize,
    inspection_issues: usize,
    issues: Vec<crate::core::Issue>,
    pins: Vec<Pinned>,
    application: Application,
    consent: ConsentQueue,
    #[cfg(target_os = "macos")]
    references: Vec<super::reference::Reference>,
}
impl Review {
    pub fn prepare(plan: &Plan) -> anyhow::Result<Self> {
        plan.validate(&config::data_roots())?;
        anyhow::ensure!(
            config::application_roots()
                .iter()
                .any(|r| plan.application.path.starts_with(r) && plan.application.path != *r),
            "application outside discovery roots"
        );
        crate::platform::macos::ensure_stopped(&plan.application.path)?;
        let app = if plan.data_only {
            ensure_absent_application(&plan.application)?;
            plan.application.clone()
        } else {
            let app = Application::read(&plan.application.path)?;
            anyhow::ensure!(
                app.bundle_id == plan.application.bundle_id,
                "application metadata changed; rescan required"
            );
            app
        };
        ensure_unshared_installation(&plan.application, plan.findings.iter().any(|f| f.selected))?;
        let mut paths = if plan.data_only {
            vec![]
        } else {
            vec![plan.application.path.clone()]
        };
        paths.extend(
            plan.findings
                .iter()
                .filter(|f| f.selected)
                .map(|f| f.path.clone()),
        );
        anyhow::ensure!(!paths.is_empty(), "select at least one remaining data path");
        for (i, path) in paths.iter().enumerate() {
            anyhow::ensure!(
                !paths[..i]
                    .iter()
                    .any(|p| path.starts_with(p) || p.starts_with(path)),
                "overlapping cleanup paths"
            );
        }
        let pins = paths
            .iter()
            .map(|p| Pinned::capture(p))
            .collect::<anyhow::Result<Vec<_>>>()?;
        for finding in plan.findings.iter().filter(|f| f.selected) {
            validate_evidence(&app, finding)?;
        }
        #[cfg(target_os = "macos")]
        let references = paths
            .iter()
            .zip(&pins)
            .map(|(path, pin)| super::reference::Reference::capture(path, pin))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let consent = ConsentQueue::from_plan(plan);
        let summary = plan.summary();
        Ok(Self {
            data_only: plan.data_only,
            data_findings: plan
                .findings
                .iter()
                .filter(|f| f.selected)
                .cloned()
                .collect(),
            paths,
            pins,
            consent,
            #[cfg(target_os = "macos")]
            references,
            application: plan.application.clone(),
            remaining: summary.remaining,
            inspection_issues: summary.inspection_issues,
            issues: plan.issues.clone(),
        })
    }
    pub fn revalidate(&self) -> anyhow::Result<()> {
        for finding in &self.data_findings {
            validate_evidence(&self.application, finding)?;
        }
        for pin in &self.pins {
            pin.revalidate()?;
        }
        if self.data_only {
            return ensure_absent_application(&self.application);
        }
        let app = Application::read(&self.application.path)?;
        anyhow::ensure!(
            app.bundle_id == self.application.bundle_id,
            "bundle metadata changed"
        );
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
    pub fn next_consent(&self) -> Option<&ManualConsent> {
        self.consent.pending.front()
    }
    pub fn approve_path(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        self.revalidate()?;
        self.consent.approve(path)
    }
    pub fn keep_path(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        let index = self
            .paths
            .iter()
            .position(|p| p == path)
            .context("path absent from review")?;
        anyhow::ensure!(
            self.data_only || index > 0,
            "application bundle cannot be skipped through ownership review"
        );
        self.consent.approve(path)?;
        self.data_findings.retain(|f| f.path != path);
        self.paths.remove(index);
        self.pins.remove(index);
        #[cfg(target_os = "macos")]
        self.references.remove(index);
        self.remaining = self.remaining.saturating_add(1);
        Ok(())
    }
    pub fn text(&self, running_status: &str) -> String {
        let confirmation = if self.paths.is_empty() {
            "All remaining paths were kept on disk. Nothing will be moved.".into()
        } else if let Some(item) = self.next_consent() {
            format!("Ownership decision\n\nPath to decide: {}\nConfidence: {:?} — ownership is not verified\nMatch evidence: {}\n\nInclude this folder in cleanup, or keep it on disk?\nOther app versions may use it.\nPaths left to review: {}", item.path.display(), item.confidence, item.reason, self.consent.pending.len())
        } else {
            "These items will be moved to macOS Trash after you confirm.\nYou can cancel without changing any files.".into()
        };
        let issues = if self.issues.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nCould not fully check:\n{}",
                self.issues
                    .iter()
                    .map(|i| format!("  {}\n    {}", i.path.display(), i.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        format!(
            "Your files have not been changed.\n\n{}\n\nTo move to Trash: {} items\n{}\n\n{}\n{}{}",
            confirmation,
            self.paths.len(),
            self.paths
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n"),
            review_summary(self.remaining, self.inspection_issues),
            running_status,
            issues
        )
    }
}

fn review_summary(remaining: usize, issues: usize) -> String {
    let kept = if remaining == 0 {
        "All data found in this scan is included. Other leftovers may still exist.".to_owned()
    } else {
        format!("Keeping on disk: {remaining} data paths found in this scan.")
    };
    let inspection = if issues == 0 {
        "No access or inspection problems were reported. The scan cannot find every possible leftover.".to_owned()
    } else {
        format!("Warning: {issues} inspection problems. Some locations or files could not be fully checked.\nReview the problems below before confirming; more data may remain on disk.")
    };
    format!("{kept}\n{inspection}")
}

impl Review {
    pub fn execute(self) -> anyhow::Result<super::trash::Outcome> {
        self.consent.ensure_complete()?;
        anyhow::ensure!(
            !self.paths.is_empty(),
            "all remaining paths were kept; nothing to move"
        );
        anyhow::ensure!(crate::platform::macos::supported(), "macOS required");
        self.revalidate()?;
        ensure_unshared_installation(&self.application, self.data_only || self.paths.len() > 1)?;
        crate::platform::macos::ensure_stopped(&self.application.path)?;
        #[cfg(target_os = "macos")]
        {
            let mut journal = super::history::Writer::start(
                &self.application,
                &self.paths,
                self.remaining,
                self.inspection_issues,
                self.data_only,
            )
            .context("cannot create cleanup journal; no files moved")?;
            let mut history_error = None;
            let mut results = vec![];
            let mut stopped = false;
            for ((path, pin), reference) in self.paths.iter().zip(&self.pins).zip(&self.references)
            {
                use super::trash::{ItemResult, MoveStatus};
                if stopped {
                    results.push(ItemResult {
                        identity: None,
                        source: path.clone(),
                        status: MoveStatus::Skipped,
                        destination: None,
                        error: Some("skipped after earlier failure or unverified move".into()),
                    });
                    continue;
                }
                let result =
                    match journal.append(&super::history::Event::Attempt { path: path.clone() }) {
                        Ok(()) => crate::platform::macos::ensure_stopped(&self.application.path)
                            .and_then(|()| {
                                if let Some(finding) =
                                    self.data_findings.iter().find(|f| f.path == *path)
                                {
                                    validate_evidence(&self.application, finding)?;
                                }
                                reference.trash(pin)
                            }),
                        Err(e) => {
                            history_error = Some(format!("{e:#}"));
                            Err(anyhow::anyhow!(
                                "intent could not be journaled; this path was not moved"
                            ))
                        }
                    };
                let item = match result {
                    Ok(receipt) => ItemResult {
                        identity: pin.identity().ok(),
                        source: path.clone(),
                        status: receipt.status,
                        destination: receipt.destination,
                        error: receipt.warning,
                    },
                    Err(e) => ItemResult {
                        identity: None,
                        source: path.clone(),
                        status: MoveStatus::Failed,
                        destination: None,
                        error: Some(format!("{e:#}")),
                    },
                };
                if history_error.is_none() {
                    if let Err(e) =
                        journal.append(&super::history::Event::Result { item: item.clone() })
                    {
                        history_error = Some(format!("{e:#}"));
                    }
                }
                stopped = item.status != MoveStatus::Moved || history_error.is_some();
                results.push(item);
            }
            if history_error.is_none() {
                // Record skipped paths as well; attempts without results remain ambiguous.
                for item in results
                    .iter()
                    .filter(|item| item.status == super::trash::MoveStatus::Skipped)
                {
                    if let Err(e) =
                        journal.append(&super::history::Event::Result { item: item.clone() })
                    {
                        history_error = Some(format!("{e:#}"));
                        break;
                    }
                }
            }
            if history_error.is_none() {
                if let Err(e) = journal.append(&super::history::Event::Finished) {
                    history_error = Some(format!("{e:#}"));
                }
            }
            let mut outcome = super::trash::Outcome::from_results_with_kind(
                results,
                self.remaining,
                self.inspection_issues,
                !self.data_only,
            );
            outcome.history_id = Some(journal.id);
            outcome.history_error = history_error;
            Ok(outcome)
        }
        #[cfg(not(target_os = "macos"))]
        anyhow::bail!("macOS required")
    }
}

fn validate_evidence(
    app: &Application,
    finding: &crate::core::scanner::Finding,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !finding.review_only
            && finding
                .path
                .parent()
                .and_then(std::path::Path::file_name)
                .is_none_or(|name| name != "LaunchAgents"),
        "LaunchAgents are inspect-only until loaded service handling is implemented"
    );
    let evidence = crate::core::association::for_path(app, &finding.path)?
        .context("association evidence disappeared")?;
    anyhow::ensure!(
        !evidence.shared,
        "shared or unknown container cannot be removed"
    );
    anyhow::ensure!(
        evidence.confidence == finding.confidence && evidence.reason == finding.reason,
        "association evidence changed; rescan required"
    );
    Ok(())
}

fn ensure_absent_source(path: &std::path::Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => anyhow::bail!(
            "application path exists or cannot be inspected; use a fresh normal application scan"
        ),
    }
    Ok(())
}
fn ensure_absent_application(app: &Application) -> anyhow::Result<()> {
    ensure_absent_source(&app.path)?;
    let installed = crate::core::discovery::discover(
        &config::application_roots(),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    anyhow::ensure!(
        !installed
            .items
            .iter()
            .any(|other| other.bundle_id == app.bundle_id),
        "an installed application uses this bundle ID; remaining data must be preserved"
    );
    Ok(())
}

fn ensure_unshared_installation(
    application: &Application,
    selected_data: bool,
) -> anyhow::Result<()> {
    if !selected_data {
        return Ok(());
    }
    let installed = crate::core::discovery::discover(
        &config::application_roots(),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    anyhow::ensure!(!installed.items.iter().any(|app| app.bundle_id == application.bundle_id && app.path != application.path), "another installation uses this bundle ID; unselect all data to preserve shared application state");
    Ok(())
}

#[derive(Clone)]
pub struct ManualConsent {
    pub path: PathBuf,
    pub confidence: crate::core::association::Confidence,
    pub reason: String,
}
#[derive(Default)]
struct ConsentQueue {
    pending: std::collections::VecDeque<ManualConsent>,
}
impl ConsentQueue {
    fn from_plan(plan: &Plan) -> Self {
        Self {
            pending: plan
                .findings
                .iter()
                .filter(|f| {
                    f.selected
                        && (plan.data_only
                            || f.confidence != crate::core::association::Confidence::High)
                })
                .map(|f| ManualConsent {
                    path: f.path.clone(),
                    confidence: f.confidence,
                    reason: f.reason.clone(),
                })
                .collect(),
        }
    }
    fn approve(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.pending.front().is_some_and(|item| item.path == path),
            "approval does not match the pending path"
        );
        self.pending.pop_front();
        Ok(())
    }
    fn ensure_complete(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.pending.is_empty(),
            "selected Medium/Low paths require individual ownership approval before execution"
        );
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::association::Confidence;
    #[test]
    fn each_uncertain_path_requires_its_own_exact_approval() {
        let mut queue = ConsentQueue {
            pending: [
                ManualConsent {
                    path: "/fixture/one".into(),
                    confidence: Confidence::Medium,
                    reason: "name".into(),
                },
                ManualConsent {
                    path: "/fixture/two".into(),
                    confidence: Confidence::Low,
                    reason: "heuristic".into(),
                },
            ]
            .into(),
        };
        assert!(queue.ensure_complete().is_err());
        assert!(queue.approve(std::path::Path::new("/fixture/two")).is_err());
        assert_eq!(queue.pending.len(), 2);
        queue.approve(std::path::Path::new("/fixture/one")).unwrap();
        assert!(queue.ensure_complete().is_err());
        assert!(queue.approve(std::path::Path::new("/fixture/one")).is_err());
        queue.approve(std::path::Path::new("/fixture/two")).unwrap();
        assert!(queue.ensure_complete().is_ok());
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn keeping_uncertain_path_removes_it_from_the_pinned_plan_without_moving_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bundle = root.join("app");
        let data = root.join("data");
        std::fs::write(&bundle, "bundle fixture").unwrap();
        std::fs::write(&data, "keep me").unwrap();
        let paths = vec![bundle.clone(), data.clone()];
        let pins = paths
            .iter()
            .map(|path| Pinned::capture(path).unwrap())
            .collect::<Vec<_>>();
        let references = paths
            .iter()
            .zip(&pins)
            .map(|(path, pin)| super::super::reference::Reference::capture(path, pin).unwrap())
            .collect();
        let mut review = Review {
            data_only: false,
            data_findings: vec![],
            paths,
            pins,
            references,
            remaining: 0,
            inspection_issues: 0,
            issues: vec![],
            application: Application {
                path: bundle.clone(),
                name: "fixture".into(),
                bundle_id: "org.fixture".into(),
                version: None,
                protected: false,
            },
            consent: ConsentQueue {
                pending: [ManualConsent {
                    path: data.clone(),
                    confidence: Confidence::Medium,
                    reason: "name match".into(),
                }]
                .into(),
            },
        };
        review.issues.push(crate::core::Issue {
            path: data.clone(),
            message: "fixture access denied".into(),
        });
        review.inspection_issues = 1;
        let text =
            review.text("App was not running when checked. This can change after the check.");
        assert!(text.contains("To move to Trash: 2 items"));
        assert!(text.contains("Could not fully check:"));
        assert!(text.contains("fixture access denied"));
        assert!(review.keep_path(&bundle).is_err());
        review.keep_path(&data).unwrap();
        assert_eq!(review.paths, vec![bundle]);
        assert_eq!(review.pins.len(), 1);
        assert_eq!(review.references.len(), 1);
        assert_eq!(review.remaining, 1);
        assert!(review.next_consent().is_none());
        assert_eq!(std::fs::read_to_string(data).unwrap(), "keep me");
    }
    #[test]
    fn data_only_recovery_requires_new_approval_even_for_high_confidence() {
        let plan = Plan {
            application: Application {
                name: "Fixture".into(),
                bundle_id: "org.fixture".into(),
                path: "/fixture/app".into(),
                version: None,
                protected: false,
            },
            bundle_bytes: 0,
            findings: vec![crate::core::scanner::Finding {
                review_only: false,
                path: "/fixture/data".into(),
                bytes: 1,
                reason: "exact ID".into(),
                confidence: Confidence::High,
                safety: "review".into(),
                shared: false,
                selected: true,
            }],
            issues: vec![],
            data_only: true,
            execution_enabled: true,
        };
        let mut consent = ConsentQueue::from_plan(&plan);
        assert!(consent.ensure_complete().is_err());
        consent
            .approve(std::path::Path::new("/fixture/data"))
            .unwrap();
        assert!(consent.ensure_complete().is_ok());
        assert!(ConsentQueue::from_plan(&plan).ensure_complete().is_err());
    }
    #[test]
    fn reinstalled_or_symlinked_bundle_source_blocks_data_only_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Fixture.app");
        assert!(ensure_absent_source(&source).is_ok());
        std::fs::create_dir(&source).unwrap();
        assert!(ensure_absent_source(&source).is_err());
        let alias = temp.path().join("Alias.app");
        std::os::unix::fs::symlink(temp.path().join("nonexistent"), &alias).unwrap();
        assert!(ensure_absent_source(&alias).is_err());
    }
    #[test]
    fn ownership_metadata_is_rechecked_when_only_the_plist_contents_change() {
        let temp = tempfile::tempdir().unwrap();
        let container = temp.path().canonicalize().unwrap().join("Containers/UUID");
        std::fs::create_dir_all(&container).unwrap();
        let app = Application {
            name: "Fixture".into(),
            bundle_id: "org.fixture.app".into(),
            path: "/fixture/app".into(),
            version: None,
            protected: false,
        };
        let write = |identifier: &str| {
            let mut dict = plist::Dictionary::new();
            dict.insert("MCMMetadataIdentifier".into(), identifier.into());
            dict.insert("MCMMetadataContentClass".into(), 2u64.into());
            plist::Value::Dictionary(dict)
                .to_file_xml(container.join(".com.apple.containermanagerd.metadata.plist"))
                .unwrap();
        };
        write("org.fixture.app");
        let evidence = crate::core::association::for_path(&app, &container)
            .unwrap()
            .unwrap();
        let finding = crate::core::scanner::Finding {
            review_only: false,
            path: container.clone(),
            bytes: 0,
            confidence: evidence.confidence,
            reason: evidence.reason,
            safety: "review".into(),
            shared: false,
            selected: true,
        };
        assert!(validate_evidence(&app, &finding).is_ok());
        write("org.other.app");
        assert!(validate_evidence(&app, &finding).is_err());
    }
    #[test]
    fn review_summary_distinguishes_included_data_from_uninspected_data() {
        let incomplete = review_summary(0, 3);
        assert!(incomplete.contains("All data found in this scan is included"));
        assert!(incomplete.contains("Warning: 3 inspection problems"));
        assert!(incomplete.contains("more data may remain"));
        let kept = review_summary(2, 0);
        assert!(kept.contains("Keeping on disk: 2 data paths"));
        assert!(kept.contains("cannot find every possible leftover"));
        assert!(!kept.contains("Warning:"));
    }
}
