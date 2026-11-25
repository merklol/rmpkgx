use std::path::PathBuf;

pub fn application_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        "/Applications".into(),
        "/System/Applications".into(),
        "/System/Library/CoreServices".into(),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }
    roots
}
pub fn data_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    [
        "Application Support",
        "Caches",
        "Preferences",
        "Containers",
        "Group Containers",
        "Logs",
        "Saved Application State",
        "HTTPStorages",
        "WebKit",
        "LaunchAgents",
    ]
    .iter()
    .map(|p| home.join("Library").join(p))
    .collect()
}
