use serde::{Deserialize, Serialize};
use std::fs::{self, create_dir_all, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubscribedChannel {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub avatar_url: String,
}

pub struct SubscriptionStore {
    path: PathBuf,
}

impl SubscriptionStore {
    pub fn new() -> Self {
        let config_dir = glib::user_config_dir().join("youtube-rusty");
        let _ = create_dir_all(&config_dir);
        Self::with_path(config_dir.join("subscriptions.json"))
    }

    pub fn with_path(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = create_dir_all(parent);
        }
        Self { path }
    }

    pub fn load_all(&self) -> Vec<SubscribedChannel> {
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

    pub fn is_subscribed(&self, channel_id: &str) -> bool {
        self.load_all().iter().any(|c| c.id == channel_id)
    }

    pub fn subscribe(&self, channel: SubscribedChannel) {
        let mut list = self.load_all();
        if let Some(existing) = list.iter_mut().find(|c| c.id == channel.id) {
            if !channel.name.is_empty() {
                existing.name = channel.name;
            }
            if !channel.handle.is_empty() {
                existing.handle = channel.handle;
            }
            if !channel.avatar_url.is_empty() {
                existing.avatar_url = channel.avatar_url;
            }
        } else {
            list.push(channel);
        }
        self.save_all(&list);
    }

    pub fn unsubscribe(&self, channel_id: &str) {
        let mut list = self.load_all();
        list.retain(|c| c.id != channel_id);
        self.save_all(&list);
    }

    fn save_all(&self, list: &[SubscribedChannel]) {
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
