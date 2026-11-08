use serde::{Deserialize, Serialize};
use std::fs::{self, create_dir_all, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub default_quality: String,
    pub hardware_decoding: bool,
    pub external_player: String,
    pub save_history: bool,
    pub color_scheme: String,
    pub accent_color: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_quality: "360p".to_string(),
            hardware_decoding: true,
            external_player: "mpv".to_string(),
            save_history: true,
            color_scheme: "dark".to_string(),
            accent_color: "terracotta".to_string(),
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
        Self::with_path(config_dir.join("settings.json"))
    }

    pub fn with_path(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = create_dir_all(parent);
        }
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
        if let Some(parent) = self.path.parent() {
            let _ = create_dir_all(parent);
        }
        let tmp_path = self.path.with_extension("json.tmp");
        if let Ok(file) = File::create(&tmp_path) {
            let mut writer = BufWriter::new(file);
            if serde_json::to_writer_pretty(&mut writer, settings).is_ok()
                && writer.flush().is_ok()
            {
                let _ = fs::rename(&tmp_path, &self.path);
                return;
            }
        }
        let _ = fs::remove_file(&tmp_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_roundtrip() {
        let path = std::env::temp_dir().join("yr_settings_test.json");
        let _ = fs::remove_file(&path);
        let store = SettingsStore::with_path(path.clone());

        let mut s = store.load();
        assert_eq!(s.default_quality, "360p");
        assert_eq!(s.accent_color, "terracotta");

        s.default_quality = "720p".to_string();
        s.accent_color = "forest".to_string();
        store.save(&s);

        let loaded = store.load();
        assert_eq!(loaded.default_quality, "720p");
        assert_eq!(loaded.accent_color, "forest");
        let _ = fs::remove_file(&path);
    }
}
