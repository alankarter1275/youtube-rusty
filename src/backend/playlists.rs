use crate::backend::models::VideoSummary;
use serde::{Deserialize, Serialize};
use std::fs::{self, create_dir_all, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PLAYLIST_ID_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedPlaylist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub source_youtube_id: Option<String>,
    #[serde(default)]
    pub author: String,
    pub created_at: u64,
    pub videos: Vec<VideoSummary>,
}

pub struct PlaylistStore {
    path: PathBuf,
}

impl PlaylistStore {
    pub fn new() -> Self {
        let config_dir = glib::user_config_dir().join("youtube-rusty");
        let _ = create_dir_all(&config_dir);
        Self::with_path(config_dir.join("playlists.json"))
    }

    pub fn with_path(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = create_dir_all(parent);
        }
        Self { path }
    }

    pub fn load_all(&self) -> Vec<SavedPlaylist> {
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

    pub fn get(&self, id: &str) -> Option<SavedPlaylist> {
        self.load_all().into_iter().find(|p| p.id == id)
    }

    pub fn find_by_youtube_id(&self, yt_id: &str) -> Option<SavedPlaylist> {
        if yt_id.is_empty() {
            return None;
        }
        self.load_all()
            .into_iter()
            .find(|p| p.source_youtube_id.as_deref() == Some(yt_id))
    }

    pub fn create(&self, name: &str) -> SavedPlaylist {
        self.create_with_videos(name, Vec::new(), None, "")
    }

    pub fn create_with_videos(
        &self,
        name: &str,
        videos: Vec<VideoSummary>,
        source_youtube_id: Option<String>,
        author: &str,
    ) -> SavedPlaylist {
        let mut list = self.load_all();

        if let Some(ref yt_id) = source_youtube_id {
            if let Some(existing) = list
                .iter_mut()
                .find(|p| p.source_youtube_id.as_deref() == Some(yt_id.as_str()))
            {
                existing.name = sanitize_name(name);
                existing.videos = videos;
                if !author.is_empty() {
                    existing.author = author.to_string();
                }
                let updated = existing.clone();
                self.save_all(&list);
                return updated;
            }
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let seq = PLAYLIST_ID_SEQ.fetch_add(1, Ordering::Relaxed);

        let playlist = SavedPlaylist {
            id: format!("pl_{now_ms}_{seq}"),
            name: sanitize_name(name),
            source_youtube_id,
            author: author.trim().to_string(),
            created_at: now_ms / 1000,
            videos,
        };

        list.insert(0, playlist.clone());
        self.save_all(&list);
        playlist
    }

    pub fn rename(&self, playlist_id: &str, new_name: &str) -> bool {
        let clean = sanitize_name(new_name);
        let mut list = self.load_all();
        if let Some(pl) = list.iter_mut().find(|p| p.id == playlist_id) {
            pl.name = clean;
            self.save_all(&list);
            return true;
        }
        false
    }

    pub fn delete(&self, playlist_id: &str) {
        let mut list = self.load_all();
        list.retain(|p| p.id != playlist_id);
        self.save_all(&list);
    }

    /// Adds `video` to the playlist if not already present. Returns `true` if added.
    pub fn add_video(&self, playlist_id: &str, video: VideoSummary) -> bool {
        let mut list = self.load_all();
        let Some(pl) = list.iter_mut().find(|p| p.id == playlist_id) else {
            return false;
        };
        if pl.videos.iter().any(|v| v.id == video.id) {
            return false;
        }
        pl.videos.push(video);
        self.save_all(&list);
        true
    }

    pub fn remove_video_at(&self, playlist_id: &str, index: usize) -> Option<VideoSummary> {
        let mut list = self.load_all();
        if let Some(pl) = list.iter_mut().find(|p| p.id == playlist_id) {
            if index < pl.videos.len() {
                let removed = pl.videos.remove(index);
                self.save_all(&list);
                return Some(removed);
            }
        }
        None
    }

    /// Restores a removed video at its former position, or at the end if the playlist changed.
    pub fn insert_video_at(
        &self,
        playlist_id: &str,
        index: usize,
        video: VideoSummary,
    ) -> bool {
        let mut list = self.load_all();
        let Some(pl) = list.iter_mut().find(|p| p.id == playlist_id) else {
            return false;
        };
        if pl.videos.iter().any(|existing| existing.id == video.id) {
            return false;
        }
        pl.videos.insert(index.min(pl.videos.len()), video);
        self.save_all(&list);
        true
    }

    pub fn move_video(&self, playlist_id: &str, from_idx: usize, to_idx: usize) -> bool {
        let mut list = self.load_all();
        let Some(pl) = list.iter_mut().find(|p| p.id == playlist_id) else {
            return false;
        };
        let len = pl.videos.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }
        let item = pl.videos.remove(from_idx);
        pl.videos.insert(to_idx, item);
        self.save_all(&list);
        true
    }

    fn save_all(&self, list: &[SavedPlaylist]) {
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

fn sanitize_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "Untitled Playlist".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_video(id: &str, title: &str) -> VideoSummary {
        VideoSummary {
            id: id.to_string(),
            title: title.to_string(),
            channel_id: String::new(),
            channel_name: "Test".to_string(),
            channel_avatar_url: String::new(),
            duration_seconds: Some(120),
            duration_text: "2:00".to_string(),
            view_count_text: String::new(),
            view_count: None,
            published_text: String::new(),
            published_unix: None,
            is_short: false,
            thumbnail_url: String::new(),
            description_snippet: String::new(),
            collaborators: Vec::new(),
        }
    }

    #[test]
    fn test_playlist_lifecycle() {
        let path = std::env::temp_dir().join("yr_playlists_test.json");
        let _ = fs::remove_file(&path);
        let store = PlaylistStore::with_path(path.clone());

        let pl = store.create("Chill Coding");
        assert_eq!(pl.name, "Chill Coding");
        assert!(store.add_video(&pl.id, dummy_video("v1", "Track 1")));
        assert!(!store.add_video(&pl.id, dummy_video("v1", "Track 1")));
        assert!(store.add_video(&pl.id, dummy_video("v2", "Track 2")));

        assert!(store.move_video(&pl.id, 1, 0));
        let loaded = store.get(&pl.id).unwrap();
        assert_eq!(loaded.videos[0].id, "v2");
        assert_eq!(loaded.videos[1].id, "v1");

        assert!(store.rename(&pl.id, "Focus Mix"));
        assert_eq!(store.get(&pl.id).unwrap().name, "Focus Mix");

        let removed = store.remove_video_at(&pl.id, 0).unwrap();
        assert_eq!(removed.id, "v2");
        assert_eq!(store.get(&pl.id).unwrap().videos.len(), 1);
        assert!(store.insert_video_at(&pl.id, 0, removed));
        assert_eq!(store.get(&pl.id).unwrap().videos[0].id, "v2");

        store.delete(&pl.id);
        assert!(store.load_all().is_empty());
        let _ = fs::remove_file(&path);
    }
}
