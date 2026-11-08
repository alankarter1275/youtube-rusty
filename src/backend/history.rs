use crate::backend::models::VideoSummary;
use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, File};
use std::io::{BufReader, BufWriter};
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
        let path = config_dir.join("history.json");

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
        // Remove existing if present to move to top
        list.retain(|item| item.video.id != video.id);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        list.insert(0, HistoryEntry {
            video,
            watched_at: now,
        });

        // Cap at 200 items
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
        if let Ok(file) = File::create(&self.path) {
            let writer = BufWriter::new(file);
            let _ = serde_json::to_writer_pretty(writer, list);
        }
    }
}
