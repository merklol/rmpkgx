use crate::core::{application::Application, scanner::Finding, Issue};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub application: Application,
    pub bundle_bytes: u64,
    pub findings: Vec<Finding>,
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub data_only: bool,
    pub execution_enabled: bool,
}
impl Plan {
    pub fn validate(&self, roots: &[std::path::PathBuf]) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.application.protected,
            "system application is protected"
        );
        if !self.data_only {
            super::safety::validate(&self.application.path)?;
        }
        for f in self.findings.iter().filter(|f| f.selected) {
            anyhow::ensure!(
                !f.shared && !f.review_only,
                "shared or inspect-only data cannot be selected"
            );
            anyhow::ensure!(
                roots.iter().any(|r| f.path.starts_with(r) && f.path != *r),
                "finding is outside data roots"
            );
            super::safety::validate(&f.path)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub selected: usize,
    pub selected_bytes: u64,
    pub remaining: usize,
    pub shared: usize,
    pub inspection_issues: usize,
}
impl Plan {
    pub fn summary(&self) -> Summary {
        let selected = self.findings.iter().filter(|f| f.selected).count();
        Summary {
            selected,
            selected_bytes: self
                .findings
                .iter()
                .filter(|f| f.selected)
                .fold(0u64, |sum, f| sum.saturating_add(f.bytes)),
            remaining: self.findings.len() - selected,
            shared: self.findings.iter().filter(|f| f.shared).count(),
            inspection_issues: self.issues.len(),
        }
    }
    /// Explicit review action for one entry, never a bulk heuristic selection.
    pub fn select(&mut self, index: usize, selected: bool) -> anyhow::Result<()> {
        let finding = self
            .findings
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("no finding selected"))?;
        if selected {
            anyhow::ensure!(
                !self.application.protected && !finding.shared && !finding.review_only,
                "protected, shared or inspect-only data cannot be selected"
            );
        }
        finding.selected = selected;
        Ok(())
    }
}
