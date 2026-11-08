use std::fs;
use std::path::PathBuf;

pub struct Cache {
    base_dir: PathBuf,
}

impl Cache {
    pub fn new() -> Self {
        let base_dir = dirs_base();
        let thumb_dir = base_dir.join("thumbnails");
        let _ = fs::create_dir_all(&thumb_dir);
        Self { base_dir: thumb_dir }
    }

    pub fn get_thumbnail_path(&self, id: &str) -> PathBuf {
        self.base_dir.join(format!("{id}.jpg"))
    }

    pub fn has_thumbnail(&self, id: &str) -> bool {
        self.get_thumbnail_path(id).exists()
    }

    pub fn save_thumbnail(&self, id: &str, data: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.get_thumbnail_path(id);
        fs::write(&path, data)?;
        Ok(path)
    }

    pub fn clear_all(&self) {
        if let Ok(entries) = fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn dirs_base() -> PathBuf {
    if let Some(proj_dirs) = directories::ProjectDirs::from("com", "kunal", "youtube-rusty") {
        proj_dirs.cache_dir().to_path_buf()
    } else {
        PathBuf::from("/tmp/youtube-rusty-cache")
    }
}
