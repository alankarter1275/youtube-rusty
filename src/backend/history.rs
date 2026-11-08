use crate::backend::models::VideoSummary;
use serde::{Deserialize, Serialize};
use std::fs::{self, create_dir_all, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub video: VideoSummary,
    pub watched_at: u64,
}

pub struct HistoryStore {
    path: PathBuf,
}

impl HistoryStore {
    pub fn new() -> Self {
        let config_dir = glib::user_config_dir().join("youtube-rusty");
        let _ = create_dir_all(&config_dir);
        Self::with_path(config_dir.join("history.json"))
    }

    pub fn with_path(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = create_dir_all(parent);
        }
        Self { path }
    }

    pub fn load_all(&self) -> Vec<HistoryEntry> {
        if !self.path.exists() {
            return Vec::new();
        }
        if let Ok(file) = File::open(&self.path) {
            let reader = BufReader::new(file);
            if let Ok(list) = serde_json::from_reader(reader) {
                return list;
            }
        }
        Vec::new()
    }

    pub fn add(&self, video: VideoSummary) {
        let mut list = self.load_all();
        list.retain(|item| item.video.id != video.id);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        list.insert(
            0,
            HistoryEntry {
                video,
                watched_at: now,
            },
        );

        if list.len() > 200 {
            list.truncate(200);
        }

        self.save(&list);
    }

    pub fn remove(&self, video_id: &str) {
        let mut list = self.load_all();
        list.retain(|item| item.video.id != video_id);
        self.save(&list);
    }

    pub fn clear(&self) {
        let list: Vec<HistoryEntry> = Vec::new();
        self.save(&list);
    }

    fn save(&self, list: &[HistoryEntry]) {
        if let Some(parent) = self.path.parent() {
            let _ = create_dir_all(parent);
        }
        let tmp_path = self.path.with_extension("json.tmp");
        if let Ok(file) = File::create(&tmp_path) {
            let mut writer = BufWriter::new(file);
            if serde_json::to_writer_pretty(&mut writer, list).is_ok() && writer.flush().is_ok() {
                let _ = fs::rename(&tmp_path, &self.path);
                return;
            }
        }
        let _ = fs::remove_file(&tmp_path);
    }
}
