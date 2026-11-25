use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveStatus {
    Moved,
    MovedUnverified,
    Failed,
    Skipped,
}
#[derive(Debug)]
pub struct MoveReceipt {
    pub status: MoveStatus,
    pub destination: Option<PathBuf>,
    pub warning: Option<String>,
}
impl MoveReceipt {
    pub fn after_success(
        destination: anyhow::Result<PathBuf>,
        verification: anyhow::Result<()>,
    ) -> Self {
        let mut warnings = vec![];
        let status = match verification {
            Ok(()) => MoveStatus::Moved,
            Err(e) => {
                warnings.push(format!(
                    "identity verification unavailable: {e:#}; inspect Trash before retrying"
                ));
                MoveStatus::MovedUnverified
            }
        };
        let destination = match destination {
            Ok(path) => Some(path),
            Err(e) => {
                warnings.push(format!("destination path unavailable: {e:#}"));
                None
            }
        };
        Self {
            status,
            destination,
            warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResult {
    #[serde(default)]
    pub identity: Option<super::native::FileIdentity>,
    pub source: PathBuf,
    pub status: MoveStatus,
    pub destination: Option<PathBuf>,
    pub error: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub items: Vec<ItemResult>,
    pub remaining: usize,
    pub inspection_issues: usize,
    pub history_id: Option<String>,
    pub history_error: Option<String>,
}
impl Outcome {
    pub fn from_results(
        items: Vec<ItemResult>,
        unselected_data: usize,
        inspection_issues: usize,
    ) -> Self {
        Self::from_results_with_kind(items, unselected_data, inspection_issues, true)
    }
    pub fn from_results_with_kind(
        items: Vec<ItemResult>,
        unselected_data: usize,
        inspection_issues: usize,
        bundle_included: bool,
    ) -> Self {
        let remaining = unselected_data.saturating_add(
            items
                .iter()
                .skip(usize::from(bundle_included))
                .filter(|item| matches!(item.status, MoveStatus::Failed | MoveStatus::Skipped))
                .count(),
        );
        Self {
            history_id: None,
            history_error: None,
            items,
            remaining,
            inspection_issues,
        }
    }
    pub fn text(&self) -> String {
        let mut text = self
            .items
            .iter()
            .map(|item| {
                let destination = item
                    .destination
                    .as_ref()
                    .map(|p| format!(" -> {}", p.display()))
                    .unwrap_or_default();
                let message = item
                    .error
                    .as_ref()
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default();
                let label = match item.status {
                    MoveStatus::Moved => "MOVED",
                    MoveStatus::MovedUnverified => "MOVED — VERIFICATION UNAVAILABLE",
                    MoveStatus::Failed => "FAILED",
                    MoveStatus::Skipped => "SKIPPED",
                };
                format!("{label} {}{destination}{message}", item.source.display())
            })
            .collect::<Vec<_>>()
            .join("\n");
        let uncertain = self
            .items
            .iter()
            .filter(|i| i.status == MoveStatus::MovedUnverified)
            .count();
        text.push_str(&format!("\nRemaining detected data (unselected + failed/skipped): {} · Inspection issues: {}\nDiscovery is not exhaustive. Review Trash and any failures; no permanent deletion was attempted.\nMoves requiring manual verification: {}", self.remaining, self.inspection_issues, uncertain));
        if let Some(id) = &self.history_id {
            text.push_str(&format!("\nHistory: {id}"));
        }
        if let Some(error) = &self.history_error {
            text.push_str(&format!("\nJOURNAL ERROR: {error}. Further moves stopped; inspect Trash and source paths before retrying."));
        }
        text
    }
    pub fn has_failures(&self) -> bool {
        self.history_error.is_some()
            || self
                .items
                .iter()
                .any(|item| item.status != MoveStatus::Moved)
    }
}
/// An unconfirmed Plan is never sufficient authorization.
pub fn execute(_plan: &super::plan::Plan) -> Result<(), crate::error::Error> {
    Err(crate::error::Error::TrashDisabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_report_exposes_every_failure_and_remaining_data() {
        let outcome = Outcome {
            items: vec![
                ItemResult {
                    identity: None,
                    source: "/fixture/app".into(),
                    status: MoveStatus::Moved,
                    destination: Some("/fixture/trash/app".into()),
                    error: None,
                },
                ItemResult {
                    identity: None,
                    source: "/fixture/data".into(),
                    status: MoveStatus::Failed,
                    destination: None,
                    error: Some("permission denied".into()),
                },
            ],
            history_id: None,
            history_error: None,
            remaining: 3,
            inspection_issues: 1,
        };
        assert!(outcome.has_failures());
        let text = outcome.text();
        assert!(text.contains("MOVED /fixture/app"));
        assert!(text.contains("FAILED /fixture/data: permission denied"));
        assert!(text.contains("Remaining detected data (unselected + failed/skipped): 3"));
        assert!(text.contains("Inspection issues: 1"));
    }
    #[test]
    fn native_success_with_denied_verification_is_not_a_failed_move() {
        let receipt = MoveReceipt::after_success(
            Err(std::io::Error::from_raw_os_error(1).into()),
            Err(std::io::Error::from_raw_os_error(1).into()),
        );
        assert_eq!(receipt.status, MoveStatus::MovedUnverified);
        let items = vec![
            ItemResult {
                identity: None,
                source: "/fixture/app".into(),
                status: receipt.status,
                destination: receipt.destination,
                error: receipt.warning,
            },
            ItemResult {
                identity: None,
                source: "/fixture/support".into(),
                status: MoveStatus::Skipped,
                destination: None,
                error: Some("earlier unverified move".into()),
            },
            ItemResult {
                identity: None,
                source: "/fixture/cache".into(),
                status: MoveStatus::Skipped,
                destination: None,
                error: Some("earlier unverified move".into()),
            },
        ];
        let outcome = Outcome::from_results(items, 0, 0);
        assert_eq!(outcome.remaining, 2);
        assert!(outcome.has_failures());
        let text = outcome.text();
        assert!(text.contains("MOVED — VERIFICATION UNAVAILABLE /fixture/app"));
        assert!(!text.contains("FAILED /fixture/app"));
        assert!(text.contains("SKIPPED /fixture/cache"));
    }
    #[test]
    fn denied_destination_path_does_not_undo_verified_native_success() {
        let receipt =
            MoveReceipt::after_success(Err(std::io::Error::from_raw_os_error(1).into()), Ok(()));
        assert_eq!(receipt.status, MoveStatus::Moved);
        assert!(receipt.destination.is_none());
        assert!(receipt
            .warning
            .unwrap()
            .contains("destination path unavailable"));
    }
}
