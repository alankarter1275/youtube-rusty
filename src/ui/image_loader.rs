use crate::backend::cache::Cache;
use adw::prelude::*;
use gtk4::gdk::{MemoryFormat, MemoryTexture, Texture};
use gtk4::{glib, Picture};
use image::imageops::FilterType;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;

/// Maximum decoded RGBA textures retained in memory (~10 MB total ceiling).
/// Compressed JPEGs/WebPs remain in disk cache so scrolling back re-decodes in <1 ms
/// without letting RSS climb over long sessions.
const MAX_CACHED_TEXTURES: usize = 48;

/// Maximum new GPU textures wrapped & attached in a single Wayland frame tick
/// (prevents multi-texture paint stalls on unified-memory GPUs — GNOME/gtk#8450).
const MAX_TEXTURES_PER_FRAME: usize = 2;
const FRAME_PACE_MS: u64 = 16;

type Waiter = Box<dyn FnOnce(Option<Texture>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageTarget {
    Card16x9,
    Short9x16,
    Avatar,
}

impl ImageTarget {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            ImageTarget::Card16x9 => (320, 180),
            ImageTarget::Short9x16 => (216, 384),
            ImageTarget::Avatar => (64, 64),
        }
    }

    fn cache_prefix(self) -> &'static str {
        match self {
            ImageTarget::Card16x9 => "16x9",
            ImageTarget::Short9x16 => "9x16",
            ImageTarget::Avatar => "av",
        }
    }
}

#[derive(Debug)]
pub struct DecodedRgba {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// Pure worker-thread image decode + aspect-fill resize. No GTK/GDK types touched.
pub fn decode_and_scale_rgba(bytes: &[u8], target: ImageTarget) -> Option<DecodedRgba> {
    if bytes.is_empty() {
        return None;
    }
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = target.dimensions();
    let scaled = img.resize_to_fill(w, h, FilterType::Triangle);
    let rgba = scaled.to_rgba8().into_raw();
    Some(DecodedRgba {
        width: w as i32,
        height: h as i32,
        rgba,
    })
}

struct LruCache<V> {
    cap: usize,
    order: VecDeque<String>,
    map: HashMap<String, V>,
}

impl<V: Clone> LruCache<V> {
    fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: VecDeque::with_capacity(cap),
            map: HashMap::with_capacity(cap),
        }
    }

    fn get(&mut self, key: &str) -> Option<V> {
        let val = self.map.get(key)?.clone();
        if self.order.back().map(|k| k.as_str()) != Some(key) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.to_string());
        }
        Some(val)
    }

    fn insert(&mut self, key: String, val: V) {
        if self.map.contains_key(&key) {
            self.order.retain(|k| k != &key);
        } else {
            while self.map.len() >= self.cap {
                if let Some(oldest) = self.order.pop_front() {
                    self.map.remove(&oldest);
                } else {
                    break;
                }
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, val);
    }

    fn clear(&mut self) {
        self.order.clear();
        self.map.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

pub struct ImageLoader {
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
    http: reqwest::Client,
    net_sem: Arc<Semaphore>,
    decode_sem: Arc<Semaphore>,
    textures: RefCell<LruCache<Texture>>,
    inflight: RefCell<HashMap<String, Vec<Waiter>>>,
    ready_queue: RefCell<VecDeque<(String, Option<DecodedRgba>)>>,
    pacer_running: Cell<bool>,
}

impl ImageLoader {
    pub fn new(cache: Arc<Cache>, runtime: Arc<Runtime>, http: reqwest::Client) -> Rc<Self> {
        Rc::new(Self {
            cache,
            runtime,
            http,
            net_sem: Arc::new(Semaphore::new(4)),
            decode_sem: Arc::new(Semaphore::new(2)),
            textures: RefCell::new(LruCache::new(MAX_CACHED_TEXTURES)),
            inflight: RefCell::new(HashMap::new()),
            ready_queue: RefCell::new(VecDeque::new()),
            pacer_running: Cell::new(false),
        })
    }

    pub fn clear_all(&self) {
        self.textures.borrow_mut().clear();
        self.cache.clear_all();
    }

    pub fn load_target(
        self: &Rc<Self>,
        disk_key: &str,
        url: &str,
        target: ImageTarget,
        on_done: impl FnOnce(Option<Texture>) + 'static,
    ) {
        if disk_key.is_empty() || url.is_empty() {
            on_done(None);
            return;
        }

        let mem_key = format!("{}_{}", target.cache_prefix(), disk_key);

        if let Some(tex) = self.textures.borrow_mut().get(&mem_key) {
            on_done(Some(tex));
            return;
        }

        {
            let mut inflight = self.inflight.borrow_mut();
            if let Some(waiters) = inflight.get_mut(&mem_key) {
                waiters.push(Box::new(on_done));
                return;
            }
            inflight.insert(mem_key.clone(), vec![Box::new(on_done)]);
        }

        let (tx, rx) = async_channel::bounded::<Option<DecodedRgba>>(1);
        let cache = self.cache.clone();
        let http = self.http.clone();
        let net_sem = self.net_sem.clone();
        let decode_sem = self.decode_sem.clone();
        let disk_key_owned = disk_key.to_string();
        let url_owned = url.to_string();

        self.runtime.spawn(async move {
            let raw_bytes: Option<Vec<u8>> = {
                let _permit = net_sem.acquire_owned().await.ok();
                let from_disk = if cache.has_thumbnail(&disk_key_owned) {
                    tokio::fs::read(cache.get_thumbnail_path(&disk_key_owned))
                        .await
                        .ok()
                        .filter(|b| !b.is_empty())
                } else {
                    None
                };

                if let Some(b) = from_disk {
                    Some(b)
                } else {
                    match http.get(&url_owned).send().await {
                        Ok(resp) => match resp.error_for_status() {
                            Ok(ok_resp) => match ok_resp.bytes().await {
                                Ok(bytes) if !bytes.is_empty() => {
                                    let vec_bytes = bytes.to_vec();
                                    let _ = cache.save_thumbnail(&disk_key_owned, &vec_bytes);
                                    Some(vec_bytes)
                                }
                                _ => None,
                            },
                            Err(_) => None,
                        },
                        Err(_) => None,
                    }
                }
            };

            let decoded = match raw_bytes {
                Some(bytes) => {
                    let _dec_permit = decode_sem.acquire_owned().await.ok();
                    tokio::task::spawn_blocking(move || decode_and_scale_rgba(&bytes, target))
                        .await
                        .ok()
                        .flatten()
                }
                None => None,
            };

            let _ = tx.send(decoded).await;
        });

        let loader_weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let decoded_opt = rx.recv().await.ok().flatten();
            let Some(loader) = loader_weak.upgrade() else {
                return;
            };
            loader
                .ready_queue
                .borrow_mut()
                .push_back((mem_key, decoded_opt));
            loader.ensure_frame_pacer();
        });
    }

    fn ensure_frame_pacer(self: &Rc<Self>) {
        if self.pacer_running.get() {
            return;
        }
        self.pacer_running.set(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(FRAME_PACE_MS), move || {
            let Some(loader) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            loader.drain_frame_batch();
            if loader.ready_queue.borrow().is_empty() {
                loader.pacer_running.set(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn drain_frame_batch(&self) {
        for _ in 0..MAX_TEXTURES_PER_FRAME {
            let Some((mem_key, decoded_opt)) = self.ready_queue.borrow_mut().pop_front() else {
                break;
            };

            let texture_opt = decoded_opt.map(|pixels| {
                let stride = pixels.width as usize * 4;
                let gbytes = glib::Bytes::from_owned(pixels.rgba);
                let tex: Texture = MemoryTexture::new(
                    pixels.width,
                    pixels.height,
                    MemoryFormat::R8g8b8a8,
                    &gbytes,
                    stride,
                )
                .upcast();
                self.textures
                    .borrow_mut()
                    .insert(mem_key.clone(), tex.clone());
                tex
            });

            let waiters = self
                .inflight
                .borrow_mut()
                .remove(&mem_key)
                .unwrap_or_default();
            for waiter in waiters {
                waiter(texture_opt.clone());
            }
        }
    }

    pub fn load_into_picture(self: &Rc<Self>, key: &str, url: &str, picture: &Picture) {
        if url.is_empty() {
            return;
        }
        let pic_weak = picture.downgrade();
        self.load_target(key, url, ImageTarget::Card16x9, move |tex_opt| {
            if let (Some(pic), Some(tex)) = (pic_weak.upgrade(), tex_opt) {
                pic.set_paintable(Some(&tex));
            }
        });
    }

    pub fn load_into_short_picture(self: &Rc<Self>, key: &str, url: &str, picture: &Picture) {
        if url.is_empty() {
            return;
        }
        let pic_weak = picture.downgrade();
        self.load_target(key, url, ImageTarget::Short9x16, move |tex_opt| {
            if let (Some(pic), Some(tex)) = (pic_weak.upgrade(), tex_opt) {
                pic.set_paintable(Some(&tex));
            }
        });
    }

    pub fn load_into_avatar(self: &Rc<Self>, channel_id: &str, url: &str, avatar: &adw::Avatar) {
        if url.is_empty() {
            return;
        }
        let cache_key = if !channel_id.is_empty() {
            format!("avatar_{channel_id}")
        } else {
            format!(
                "avatar_url_{}",
                crate::backend::cache::sanitize_cache_key(url)
            )
        };
        let av_weak = avatar.downgrade();
        self.load_target(&cache_key, url, ImageTarget::Avatar, move |tex_opt| {
            if let (Some(av), Some(tex)) = (av_weak.upgrade(), tex_opt) {
                av.set_custom_image(Some(&tex));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    #[test]
    fn test_lru_cache_eviction_and_recency() {
        let mut lru = LruCache::new(3);
        lru.insert("a".to_string(), 1);
        lru.insert("b".to_string(), 2);
        lru.insert("c".to_string(), 3);
        assert_eq!(lru.len(), 3);

        // Touch "a" so "b" becomes the oldest
        assert_eq!(lru.get("a"), Some(1));

        // Inserting "d" must evict "b", keeping "a", "c", "d"
        lru.insert("d".to_string(), 4);
        assert_eq!(lru.len(), 3);
        assert_eq!(lru.get("b"), None);
        assert_eq!(lru.get("a"), Some(1));
        assert_eq!(lru.get("c"), Some(3));
        assert_eq!(lru.get("d"), Some(4));
    }

    #[test]
    fn test_off_thread_decode_and_scale() {
        let img = RgbaImage::from_pixel(640, 480, Rgba([212, 85, 42, 255]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();

        let card = decode_and_scale_rgba(buf.get_ref(), ImageTarget::Card16x9).unwrap();
        assert_eq!(card.width, 320);
        assert_eq!(card.height, 180);
        assert_eq!(card.rgba.len(), 320 * 180 * 4);

        let short = decode_and_scale_rgba(buf.get_ref(), ImageTarget::Short9x16).unwrap();
        assert_eq!(short.width, 216);
        assert_eq!(short.height, 384);
        assert_eq!(short.rgba.len(), 216 * 384 * 4);
    }
}
