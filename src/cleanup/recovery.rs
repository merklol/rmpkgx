//! Recovery only considers explicitly failed/skipped data paths with fresh
//! filesystem evidence. Old approvals and ambiguous move outcomes never carry.
use super::{
    history::{Event, Journal},
    plan::Plan,
    trash::MoveStatus,
};
use crate::core::scanner;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

pub fn candidates(journal: &Journal) -> anyhow::Result<BTreeSet<PathBuf>> {
    anyhow::ensure!(
        journal.issues.is_empty(),
        "damaged journal must be inspected manually"
    );
    anyhow::ensure!(
        !matches!(
            journal.events.first(),
            Some(Event::Started { restore: true, .. })
        ),
        "restore journal cannot authorize cleanup recovery"
    );
    let application = journal.application()?;
    let data_only = matches!(
        journal.events.first(),
        Some(Event::Started {
            data_only: true,
            ..
        })
    );
    let mut eligible = BTreeSet::new();
    let mut excluded: BTreeSet<_> = journal.unresolved_attempts().into_iter().collect();
    let mut bundle_moved = false;
    for event in &journal.events {
        if let Event::Result { item } = event {
            if item.source == application.path && item.status == MoveStatus::Moved {
                bundle_moved = true;
            }
            match item.status {
                MoveStatus::Failed | MoveStatus::Skipped => {
                    eligible.insert(item.source.clone());
                }
                MoveStatus::Moved | MoveStatus::MovedUnverified => {
                    excluded.insert(item.source.clone());
                }
            }
        }
    }
    anyhow::ensure!(
        data_only || bundle_moved,
        "bundle move was not verified; inspect source and Trash manually before recovery"
    );
    eligible.remove(&application.path);
    let approved = match journal.events.first() {
        Some(Event::Started { paths, .. }) => paths,
        _ => unreachable!("header checked"),
    };
    eligible.retain(|path| !excluded.contains(path) && approved.contains(path));
    Ok(eligible)
}
pub fn scan(
    journal: &Journal,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Path),
) -> anyhow::Result<Plan> {
    let eligible = candidates(journal)?;
    let app = journal.application()?.clone();
    let report = scanner::scan_with_progress(&app, &crate::config::data_roots(), cancel, progress)?;
    plan_from_scan(journal, report, &eligible)
}
fn plan_from_scan(
    journal: &Journal,
    mut report: crate::core::Report<scanner::Finding>,
    eligible: &BTreeSet<PathBuf>,
) -> anyhow::Result<Plan> {
    let app = journal.application()?.clone();
    report
        .items
        .retain(|finding| eligible.contains(&finding.path));
    for path in eligible {
        if !report.items.iter().any(|finding| finding.path == *path) {
            report.issues.push(crate::core::Issue { path: path.clone(), message: "recorded retained path is absent or no longer safely associated; excluded from cleanup".into() });
        }
    }
    for finding in &mut report.items {
        finding.selected = false;
    }
    Ok(Plan {
        application: app,
        bundle_bytes: 0,
        findings: report.items,
        issues: report.issues,
        data_only: true,
        execution_enabled: cfg!(target_os = "macos"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::trash::{ItemResult, Outcome};
    use crate::core::{application::Application, association::Confidence, Report};
    fn result(path: &str, status: MoveStatus) -> Event {
        Event::Result {
            item: ItemResult {
                identity: None,
                source: path.into(),
                status,
                destination: None,
                error: None,
            },
        }
    }
    fn journal() -> Journal {
        Journal {
            id: "123".into(),
            issues: vec![],
            events: vec![
                Event::Started {
                    version: 1,
                    restore: false,
                    data_only: false,
                    application: Application {
                        name: "Fixture".into(),
                        bundle_id: "org.fixture".into(),
                        path: "/fixture/app".into(),
                        version: None,
                        protected: false,
                    },
                    paths: [
                        "/fixture/app",
                        "/fixture/failed",
                        "/fixture/unknown",
                        "/fixture/moved",
                    ]
                    .map(PathBuf::from)
                    .to_vec(),
                    remaining: 0,
                    inspection_issues: 0,
                },
                result("/fixture/app", MoveStatus::Moved),
                result("/fixture/failed", MoveStatus::Skipped),
                result("/fixture/unknown", MoveStatus::Skipped),
                Event::Attempt {
                    path: "/fixture/unknown".into(),
                },
                result("/fixture/moved", MoveStatus::MovedUnverified),
                result("/fixture/moved", MoveStatus::Failed),
                result("/fixture/not-approved", MoveStatus::Failed),
            ],
        }
    }
    #[test]
    fn only_known_retained_approved_data_is_eligible() {
        let journal = journal();
        assert_eq!(
            candidates(&journal).unwrap(),
            [PathBuf::from("/fixture/failed")].into()
        );
    }
    #[test]
    fn ambiguous_bundle_and_corrupt_journals_cannot_authorize_recovery() {
        let mut journal = journal();
        journal.events[1] = result("/fixture/app", MoveStatus::MovedUnverified);
        assert!(candidates(&journal).is_err());
        journal.events[1] = result("/fixture/app", MoveStatus::Moved);
        journal.issues.push("truncated".into());
        assert!(candidates(&journal).is_err());
    }
    #[test]
    fn fresh_data_only_plan_discards_old_selections_and_does_not_include_the_bundle() {
        let journal = journal();
        let finding = |path: &str| scanner::Finding {
            review_only: false,
            path: path.into(),
            bytes: 4,
            confidence: Confidence::High,
            reason: "fixture".into(),
            safety: "review".into(),
            shared: false,
            selected: true,
        };
        let report = Report {
            items: vec![finding("/fixture/failed"), finding("/fixture/moved")],
            issues: vec![],
        };
        let plan = plan_from_scan(&journal, report, &candidates(&journal).unwrap()).unwrap();
        assert!(plan.data_only);
        assert_eq!(plan.bundle_bytes, 0);
        assert_eq!(plan.findings.len(), 1);
        assert!(!plan.findings[0].selected);
        let outcome = Outcome::from_results_with_kind(
            vec![ItemResult {
                identity: None,
                source: "/fixture/failed".into(),
                status: MoveStatus::Skipped,
                destination: None,
                error: None,
            }],
            0,
            0,
            false,
        );
        assert_eq!(outcome.remaining, 1);
    }
}
