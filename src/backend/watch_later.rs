use crate::backend::models::VideoSummary;
use std::fs::{self, create_dir_all, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

pub struct WatchLaterStore {
    path: PathBuf,
}

impl WatchLaterStore {
    pub fn new() -> Self {
        let config_dir = glib::user_config_dir().join("youtube-rusty");
        let _ = create_dir_all(&config_dir);
        Self::with_path(config_dir.join("watch_later.json"))
    }

    pub fn with_path(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = create_dir_all(parent);
        }
        Self { path }
    }

    pub fn load_all(&self) -> Vec<VideoSummary> {
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

    pub fn contains(&self, video_id: &str) -> bool {
        self.load_all().iter().any(|v| v.id == video_id)
    }

    /// Toggles the video in Watch Later. Returns `true` if saved, `false` if removed.
    pub fn toggle(&self, video: VideoSummary) -> bool {
        let mut list = self.load_all();
        if list.iter().any(|v| v.id == video.id) {
            list.retain(|v| v.id != video.id);
            self.save_all(&list);
            false
        } else {
            list.insert(0, video);
            if list.len() > 300 {
                list.truncate(300);
            }
            self.save_all(&list);
            true
        }
    }

    pub fn clear(&self) {
        self.save_all(&[]);
    }

    fn save_all(&self, list: &[VideoSummary]) {
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
