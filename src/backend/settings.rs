use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, File};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub default_quality: String,
    pub hardware_decoding: bool,
    pub external_player: String,
    pub save_history: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_quality: "360p".to_string(),
            hardware_decoding: true,
            external_player: "mpv".to_string(),
            save_history: true,
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new() -> Self {
        let config_dir = glib::user_config_dir().join("youtube-rusty");
        let _ = create_dir_all(&config_dir);
        let path = config_dir.join("settings.json");

        Self { path }
    }

    pub fn load(&self) -> AppSettings {
        if !self.path.exists() {
            return AppSettings::default();
        }
        if let Ok(file) = File::open(&self.path) {
            let reader = BufReader::new(file);
            if let Ok(settings) = serde_json::from_reader(reader) {
                return settings;
            }
        }
        AppSettings::default()
    }

    pub fn save(&self, settings: &AppSettings) {
        if let Ok(file) = File::create(&self.path) {
            let writer = BufWriter::new(file);
            let _ = serde_json::to_writer_pretty(writer, settings);
        }
    }
}
