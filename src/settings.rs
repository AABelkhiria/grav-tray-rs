use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

pub const DEFAULT_REFRESH_INTERVAL: u64 = 60;
pub const VALID_REFRESH_INTERVALS: [u64; 4] = [30, 60, 300, 900];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub refresh_interval_seconds: u64,
    pub menu_bar_quota: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_interval_seconds: DEFAULT_REFRESH_INTERVAL,
            menu_bar_quota: String::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        let Ok(data) = fs::read(path) else {
            return Self::default();
        };
        let Ok(mut settings) = serde_json::from_slice::<Self>(&data) else {
            return Self::default();
        };
        if !VALID_REFRESH_INTERVALS.contains(&settings.refresh_interval_seconds) {
            settings.refresh_interval_seconds = DEFAULT_REFRESH_INTERVAL;
        }
        settings
    }

    pub fn save(&self) -> io::Result<()> {
        let path = settings_path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(path, data)
    }
}

pub fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join(env!("CARGO_PKG_NAME")).join("config.json"))
}
