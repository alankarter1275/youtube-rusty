use std::fs::{self, create_dir_all, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

const MAX_SEARCH_HISTORY: usize = 25;

pub struct SearchHistoryStore {
    path: PathBuf,
}

impl SearchHistoryStore {
    pub fn new() -> Self {
        let config_dir = glib::user_config_dir().join("youtube-rusty");
        let _ = create_dir_all(&config_dir);
        Self::with_path(config_dir.join("search_history.json"))
    }

    pub fn with_path(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = create_dir_all(parent);
        }
        Self { path }
    }

    pub fn load_all(&self) -> Vec<String> {
        if !self.path.exists() {
            return Vec::new();
        }
        if let Ok(file) = File::open(&self.path) {
            let reader = BufReader::new(file);
            if let Ok(list) = serde_json::from_reader::<_, Vec<String>>(reader) {
                return list;
            }
        }
        Vec::new()
    }

    pub fn add(&self, query: &str) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut list = self.load_all();
        list.retain(|q| !q.eq_ignore_ascii_case(trimmed));
        list.insert(0, trimmed.to_string());
        if list.len() > MAX_SEARCH_HISTORY {
            list.truncate(MAX_SEARCH_HISTORY);
        }
        self.save_all(&list);
    }

    pub fn remove(&self, query: &str) {
        let mut list = self.load_all();
        list.retain(|q| q != query);
        self.save_all(&list);
    }

    pub fn clear(&self) {
        self.save_all(&[]);
    }

    fn save_all(&self, list: &[String]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_history_dedup_and_remove() {
        let path = std::env::temp_dir().join("yr_search_history_test.json");
        let _ = fs::remove_file(&path);
        let store = SearchHistoryStore::with_path(path.clone());

        store.add("rust gtk4");
        store.add("linux setup");
        store.add("Rust GTK4");

        let items = store.load_all();
        assert_eq!(items, vec!["Rust GTK4".to_string(), "linux setup".to_string()]);

        store.remove("linux setup");
        assert_eq!(store.load_all(), vec!["Rust GTK4".to_string()]);

        store.clear();
        assert!(store.load_all().is_empty());
        let _ = fs::remove_file(&path);
    }
}
