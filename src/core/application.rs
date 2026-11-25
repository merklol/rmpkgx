use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Application {
    pub name: String,
    pub bundle_id: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub protected: bool,
}
impl Application {
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
            "symlink bundle skipped"
        );
        let value = plist::Value::from_file(path.join("Contents/Info.plist"))?;
        let dict = value
            .as_dictionary()
            .ok_or_else(|| anyhow::anyhow!("Info.plist is not a dictionary"))?;
        let text = |key: &str| {
            dict.get(key)
                .and_then(plist::Value::as_string)
                .map(str::to_owned)
        };
        let bundle_id = text("CFBundleIdentifier")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing bundle identifier"))?;
        Ok(Self {
            name: text("CFBundleDisplayName")
                .or_else(|| text("CFBundleName"))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }),
            bundle_id,
            path: path.to_owned(),
            version: text("CFBundleShortVersionString").or_else(|| text("CFBundleVersion")),
            protected: crate::cleanup::safety::protected(path),
        })
    }
}
