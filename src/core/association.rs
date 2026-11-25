use super::application::Application;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

pub fn associate(app: &Application, name: &str) -> Option<(Confidence, String)> {
    if name == app.bundle_id
        || name == format!("{}.plist", app.bundle_id)
        || name == format!("{}.savedState", app.bundle_id)
    {
        Some((
            Confidence::High,
            "Exact bundle identifier in a standard data location".into(),
        ))
    } else if name.eq_ignore_ascii_case(&app.name) {
        Some((
            Confidence::Medium,
            "Exact application display name; ownership is unverified".into(),
        ))
    } else if app.name.len() >= 4
        && name
            .to_lowercase()
            .starts_with(&format!("{}.", app.name.to_lowercase()))
    {
        Some((
            Confidence::Low,
            "Filename convention only; manual ownership verification required".into(),
        ))
    } else {
        None
    }
}

/// One association policy shared by scanning and immediate pre-move checks.
pub fn for_path(
    app: &Application,
    path: &std::path::Path,
) -> anyhow::Result<Option<super::containers::Evidence>> {
    let parent = path.parent().and_then(std::path::Path::file_name);
    if parent.is_some_and(|name| name == "LaunchAgents") {
        return super::launch_agents::evaluate(app, path);
    }
    if parent.is_some_and(|name| name == "Containers" || name == "Group Containers") {
        return super::containers::evaluate(
            app,
            path,
            parent.is_some_and(|name| name == "Group Containers"),
        );
    }
    Ok(path
        .file_name()
        .and_then(|name| associate(app, &name.to_string_lossy()))
        .map(|(confidence, reason)| super::containers::Evidence {
            confidence,
            reason,
            shared: false,
            issue: None,
        }))
}
