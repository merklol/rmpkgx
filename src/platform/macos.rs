use std::path::Path;

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Best-effort snapshot only: processes can start after inspection. Never use
/// a negative result as authorization to remove files.
pub fn running_status(bundle: &Path) -> String {
    if !supported() {
        return "Running status unavailable on this platform".into();
    }
    match std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,comm="])
        .output()
    {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(text) => {
                let pids = running_pids(&text, bundle);
                if pids.is_empty() {
                    "App was not running when checked. This can change after the check.".into()
                } else {
                    format!(
                        "RUNNING — PID {}. Quit the app before any future cleanup.",
                        pids.join(", ")
                    )
                }
            }
            Err(_) => "Running status unknown: process list is not valid UTF-8".into(),
        },
        _ => "Running status unknown: process inspection failed".into(),
    }
}
fn running_pids(text: &str, bundle: &Path) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let split = line.find(char::is_whitespace)?;
            let (pid, executable) = line.split_at(split);
            if !pid.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let executable = Path::new(executable.trim_start());
            (executable.starts_with(bundle) && executable != bundle).then(|| pid.to_owned())
        })
        .collect()
}
/// Fail closed on unavailable process inspection. This remains a snapshot,
/// since another process may launch an app after the check.
pub fn ensure_stopped(bundle: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(supported(), "macOS required");
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,comm="])
        .output()?;
    anyhow::ensure!(output.status.success(), "cannot inspect running processes");
    let text = String::from_utf8(output.stdout)?;
    let pids = running_pids(&text, bundle);
    anyhow::ensure!(
        pids.is_empty(),
        "application is running (PID {}); quit it and prepare a new review",
        pids.join(", ")
    );
    Ok(())
}

/// Opens Settings only; never grants permissions or changes privacy state.
pub fn open_system_settings() -> anyhow::Result<()> {
    anyhow::ensure!(supported(), "macOS required");
    let status = std::process::Command::new("/usr/bin/open")
        .args(["-b", "com.apple.systempreferences"])
        .status()?;
    anyhow::ensure!(
        status.success(),
        "could not open System Settings; open it manually from the Apple menu"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_paths_are_component_matched() {
        let text = " 12 /Applications/My App.app/Contents/MacOS/My App\n13 /Applications/My App.app.backup/helper\n14 /Applications/Other.app/helper\n";
        assert_eq!(
            running_pids(text, Path::new("/Applications/My App.app")),
            vec!["12"]
        );
    }
}
