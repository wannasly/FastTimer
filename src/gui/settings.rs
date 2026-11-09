use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::PathBuf};

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Settings {
    pub interval: Option<String>,
    pub cases: Option<String>,
}

impl Settings {
    fn folder() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("FastTimer"))
    }

    pub fn load() -> Self {
        let Some(folder) = Self::folder() else {
            return Self::default();
        };
        let Ok(file) = fs::File::open(folder.join("settings.json")) else {
            return Self::default();
        };
        serde_json::from_reader(file.take(8192)).unwrap_or_default()
    }

    pub fn save(&self) -> fasttimer::Result<()> {
        let Some(folder) = Self::folder() else {
            return Ok(());
        };
        fs::create_dir_all(&folder)?;
        let pending = folder.join("settings.tmp");
        fs::write(&pending, serde_json::to_vec_pretty(self)?)?;
        fs::rename(pending, folder.join("settings.json"))?;
        Ok(())
    }
}
