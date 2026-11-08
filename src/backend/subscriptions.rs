use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, File};
use std::io::{BufReader, BufWriter};
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
        let path = config_dir.join("subscriptions.json");

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
        if !list.iter().any(|c| c.id == channel.id) {
            list.push(channel);
            self.save_all(&list);
        }
    }

    pub fn unsubscribe(&self, channel_id: &str) {
        let mut list = self.load_all();
        list.retain(|c| c.id != channel_id);
        self.save_all(&list);
    }

    fn save_all(&self, list: &[SubscribedChannel]) {
        if let Ok(file) = File::create(&self.path) {
            let writer = BufWriter::new(file);
            let _ = serde_json::to_writer_pretty(writer, list);
        }
    }
}
