use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct Cache {
    root_dir: PathBuf,
    thumb_dir: PathBuf,
}

impl Cache {
    pub fn new() -> Self {
        Self::with_root(dirs_base())
    }

    pub fn with_root(root_dir: PathBuf) -> Self {
        let thumb_dir = root_dir.join("thumbnails");
        let _ = fs::create_dir_all(&thumb_dir);
        Self {
            root_dir,
            thumb_dir,
        }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn get_thumbnail_path(&self, id: &str) -> PathBuf {
        let safe_id = sanitize_cache_key(id);
        self.thumb_dir.join(format!("{safe_id}.jpg"))
    }

    pub fn has_thumbnail(&self, id: &str) -> bool {
        let path = self.get_thumbnail_path(id);
        fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false)
    }

    pub fn save_thumbnail(&self, id: &str, data: &[u8]) -> std::io::Result<PathBuf> {
        let final_path = self.get_thumbnail_path(id);
        if fs::metadata(&final_path).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(final_path);
        }
        let _ = fs::create_dir_all(&self.thumb_dir);
        let safe_id = sanitize_cache_key(id);
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = self.thumb_dir.join(format!("{safe_id}.{seq}.part"));

        fs::write(&tmp_path, data)?;
        if let Err(err) = fs::rename(&tmp_path, &final_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        Ok(final_path)
    }

    /// Bounds the on-disk thumbnail directory with hysteresis so mechanical HDDs never
    /// accumulate tens of thousands of small files over months of use.
    pub fn prune_excess_files(&self, max_files: usize) {
        let Ok(rd) = fs::read_dir(&self.thumb_dir) else {
            return;
        };
        let mut files: Vec<( std::time::SystemTime, PathBuf )> = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    files.push((mtime, path));
                }
            }
        }
        if files.len() <= max_files {
            return;
        }
        files.sort_unstable_by_key(|(t, _)| *t);
        let target_keep = (max_files * 3) / 4;
        let remove_count = files.len().saturating_sub(target_keep);
        for (_, path) in files.into_iter().take(remove_count) {
            let _ = fs::remove_file(path);
        }
    }

    pub fn clear_all(&self) {
        if let Ok(entries) = fs::read_dir(&self.thumb_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

pub fn sanitize_cache_key(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "empty".to_string()
    } else {
        cleaned
    }
}

fn dirs_base() -> PathBuf {
    if let Some(proj_dirs) = directories::ProjectDirs::from("com", "kunal", "youtube-rusty") {
        proj_dirs.cache_dir().to_path_buf()
    } else {
        glib::user_cache_dir().join("youtube-rusty")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_and_atomic_save() {
        assert_eq!(sanitize_cache_key("../etc/passwd"), "___etc_passwd");
        assert_eq!(sanitize_cache_key("dQw4w9WgXcQ"), "dQw4w9WgXcQ");

        let dir = std::env::temp_dir().join("yr_cache_test");
        let _ = fs::remove_dir_all(&dir);
        let cache = Cache::with_root(dir.clone());

        assert!(!cache.has_thumbnail("vid1"));
        cache.save_thumbnail("vid1", b"jpegdata").unwrap();
        assert!(cache.has_thumbnail("vid1"));

        // Second save skips rewriting existing non-empty file
        cache.save_thumbnail("vid1", b"different").unwrap();
        assert_eq!(
            fs::read(cache.get_thumbnail_path("vid1")).unwrap(),
            b"jpegdata"
        );

        cache.clear_all();
        assert!(!cache.has_thumbnail("vid1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_excess_files_hysteresis() {
        let dir = std::env::temp_dir().join("yr_cache_prune_test");
        let _ = fs::remove_dir_all(&dir);
        let cache = Cache::with_root(dir.clone());

        for i in 0..20 {
            cache
                .save_thumbnail(&format!("thumb_{i}"), b"jpegbytes")
                .unwrap();
        }
        cache.prune_excess_files(16);
        let remaining = fs::read_dir(dir.join("thumbnails")).unwrap().count();
        assert_eq!(remaining, 12); // (16 * 3) / 4 = 12

        let _ = fs::remove_dir_all(&dir);
    }
}
