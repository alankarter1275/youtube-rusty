use crate::backend::cache::Cache;
use adw::prelude::*;
use gtk4::gdk::{MemoryFormat, MemoryTexture, Texture};
use gtk4::{glib, Picture};
use image::imageops::FilterType;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

/// Maximum decoded RGBA textures retained in the lookup LRU (~4.2 MB ceiling).
const MAX_CACHED_TEXTURES: usize = 24;

/// Strict 5 MB in-memory ring of *compressed* JPEG/WebP thumbnails (~400+ thumbnails).
/// Prevents spinning-HDD head seeks when scrolling back and forth across hundreds of cards.
const MAX_COMPRESSED_BYTES: usize = 5 * 1024 * 1024;

/// Maximum decoded `Vec<u8>` pixel buffers allowed in flight across worker threads + `ready_queue`
/// (~1.0 MB ceiling), preventing multi-thread heap arena blowup during rapid flick-scrolling.
const MAX_INFLIGHT_DECODED_BUFFERS: usize = 6;

/// Maximum on-disk cached thumbnail files before background oldest-first pruning kicks in.
const MAX_DISK_CACHE_FILES: usize = 650;

/// Maximum new GPU textures wrapped & attached in a single Wayland frame tick
/// (prevents multi-texture paint stalls on unified-memory GPUs — GNOME/gtk#8450).
const MAX_TEXTURES_PER_FRAME: usize = 2;
const FRAME_PACE_MS: u64 = 16;

struct WaiterEntry {
    is_alive: Box<dyn Fn() -> bool>,
    on_done: Box<dyn FnOnce(Option<Texture>)>,
}

struct InflightEntry {
    cancelled: Arc<AtomicBool>,
    waiters: Vec<WaiterEntry>,
}

/// Returns unused `glibc` heap pages from main + worker arenas back to Linux (`MADV_DONTNEED`)
/// so `btop` RSS stays flat instead of ratcheting upward on low-RAM systems.
pub fn trim_process_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> std::os::raw::c_int;
        }
        // SAFETY: `malloc_trim(0)` is thread-safe in glibc and simply releases free top/mmap chunks.
        unsafe {
            let _ = malloc_trim(0);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageTarget {
    Card16x9,
    Short9x16,
    Avatar,
}

impl ImageTarget {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            ImageTarget::Card16x9 => (288, 162),
            ImageTarget::Short9x16 => (158, 280),
            ImageTarget::Avatar => (56, 56),
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

    fn shrink_to(&mut self, keep: usize) {
        while self.map.len() > keep {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            } else {
                break;
            }
        }
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

/// Strict byte-bounded LRU storing compressed JPEG/WebP thumbnails (`Arc<[u8]>`) in RAM.
struct ByteBudgetLru {
    max_bytes: usize,
    used_bytes: usize,
    order: VecDeque<String>,
    map: HashMap<String, Arc<[u8]>>,
}

impl ByteBudgetLru {
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.max(1024),
            used_bytes: 0,
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<Arc<[u8]>> {
        let val = self.map.get(key)?.clone();
        if self.order.back().map(|k| k.as_str()) != Some(key) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.to_string());
        }
        Some(val)
    }

    fn insert(&mut self, key: String, val: Arc<[u8]>) {
        let sz = val.len();
        if sz == 0 || sz > self.max_bytes / 2 {
            return;
        }
        if let Some(prev) = self.map.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(prev.len());
            self.order.retain(|k| k != &key);
        }
        while self.used_bytes + sz > self.max_bytes {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(evicted) = self.map.remove(&oldest) {
                    self.used_bytes = self.used_bytes.saturating_sub(evicted.len());
                }
            } else {
                self.used_bytes = 0;
                break;
            }
        }
        self.used_bytes += sz;
        self.order.push_back(key.clone());
        self.map.insert(key, val);
    }

    fn clear(&mut self) {
        self.order.clear();
        self.map.clear();
        self.used_bytes = 0;
    }

    #[cfg(test)]
    fn used_bytes(&self) -> usize {
        self.used_bytes
    }
}

type ReadyItem = (String, Option<DecodedRgba>, Option<OwnedSemaphorePermit>);

pub struct ImageLoader {
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
    http: reqwest::Client,
    net_sem: Arc<Semaphore>,
    decode_sem: Arc<Semaphore>,
    ready_slots: Arc<Semaphore>,
    compressed_lru: Arc<Mutex<ByteBudgetLru>>,
    disk_save_tx: mpsc::Sender<(String, Arc<[u8]>)>,
    textures: RefCell<LruCache<Texture>>,
    inflight: RefCell<HashMap<String, InflightEntry>>,
    ready_queue: RefCell<VecDeque<ReadyItem>>,
    pacer_running: Cell<bool>,
    trim_scheduled: Cell<bool>,
}

impl ImageLoader {
    pub fn new(cache: Arc<Cache>, runtime: Arc<Runtime>, http: reqwest::Client) -> Rc<Self> {
        let (disk_save_tx, mut disk_save_rx) = mpsc::channel::<(String, Arc<[u8]>)>(64);
        let cache_for_writer = cache.clone();

        // Single calm background writer so fast scrolling never triggers HDD temp-file storms.
        runtime.spawn(async move {
            let mut writes_since_prune = 0usize;
            while let Some((disk_key, bytes)) = disk_save_rx.recv().await {
                let c = cache_for_writer.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = c.save_thumbnail(&disk_key, &bytes);
                })
                .await;
                writes_since_prune += 1;
                if writes_since_prune >= 160 {
                    writes_since_prune = 0;
                    let c_prune = cache_for_writer.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        c_prune.prune_excess_files(MAX_DISK_CACHE_FILES);
                    })
                    .await;
                }
                tokio::time::sleep(Duration::from_millis(35)).await;
            }
        });

        Rc::new(Self {
            cache,
            runtime,
            http,
            net_sem: Arc::new(Semaphore::new(4)),
            decode_sem: Arc::new(Semaphore::new(2)),
            ready_slots: Arc::new(Semaphore::new(MAX_INFLIGHT_DECODED_BUFFERS)),
            compressed_lru: Arc::new(Mutex::new(ByteBudgetLru::new(MAX_COMPRESSED_BYTES))),
            disk_save_tx,
            textures: RefCell::new(LruCache::new(MAX_CACHED_TEXTURES)),
            inflight: RefCell::new(HashMap::new()),
            ready_queue: RefCell::new(VecDeque::new()),
            pacer_running: Cell::new(false),
            trim_scheduled: Cell::new(false),
        })
    }

    pub fn diagnostics_summary(&self) {
        let bytes = self.compressed_lru.try_lock().map(|c| c.used_bytes).ok();
        crate::diagnostics::image_summary(self.textures.borrow().map.len(), bytes, self.inflight.borrow().len(), self.ready_queue.borrow().len());
    }

    pub fn clear_all(self: &Rc<Self>) {
        self.textures.borrow_mut().clear();
        if let Ok(mut guard) = self.compressed_lru.lock() {
            guard.clear();
        }
        self.cache.clear_all();
        self.schedule_heap_trim();
    }

    /// Shrinks the in-memory `Texture` lookup cache and schedules a debounced `malloc_trim(0)`.
    pub fn shrink_textures(self: &Rc<Self>, keep: usize) {
        self.textures.borrow_mut().shrink_to(keep);
        self.sweep_cancelled_inflight();
        self.schedule_heap_trim();
    }

    /// Coalesced 250ms idle heap trim so freed GTK/RGBA pages are returned to Linux cleanly.
    pub fn schedule_heap_trim(self: &Rc<Self>) {
        if self.trim_scheduled.get() {
            return;
        }
        self.trim_scheduled.set(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(250), move || {
            if let Some(loader) = weak.upgrade() {
                loader.trim_scheduled.set(false);
            }
            trim_process_heap();
        });
    }

    /// Marks any in-flight thumbnail load whose target widgets have all been dropped/recycled
    /// as cancelled so worker threads skip network download and CPU RGBA unpacking immediately.
    pub fn sweep_cancelled_inflight(&self) {
        let mut inflight = self.inflight.borrow_mut();
        inflight.retain(|_, entry| {
            entry.waiters.retain(|w| (w.is_alive)());
            if entry.waiters.is_empty() {
                entry.cancelled.store(true, Ordering::Relaxed);
                false
            } else {
                true
            }
        });
    }

    pub fn load_target(
        self: &Rc<Self>,
        disk_key: &str,
        url: &str,
        target: ImageTarget,
        is_alive: impl Fn() -> bool + 'static,
        on_done: impl FnOnce(Option<Texture>) + 'static,
    ) {
        if disk_key.is_empty() || url.is_empty() {
            on_done(None);
            return;
        }
        if !is_alive() {
            return;
        }

        let mem_key = format!("{}_{}", target.cache_prefix(), disk_key);

        if let Some(tex) = self.textures.borrow_mut().get(&mem_key) {
            crate::diagnostics::image_event(crate::diagnostics::ImageEvent::TextureHit, 1);
            on_done(Some(tex));
            return;
        }

        let cancelled = {
            let mut inflight = self.inflight.borrow_mut();
            if let Some(entry) = inflight.get_mut(&mem_key) {
                crate::diagnostics::image_event(crate::diagnostics::ImageEvent::Coalesced, 1);
                entry.waiters.push(WaiterEntry {
                    is_alive: Box::new(is_alive),
                    on_done: Box::new(on_done),
                });
                return;
            }
            let flag = Arc::new(AtomicBool::new(false));
            inflight.insert(
                mem_key.clone(),
                InflightEntry {
                    cancelled: flag.clone(),
                    waiters: vec![WaiterEntry {
                        is_alive: Box::new(is_alive),
                        on_done: Box::new(on_done),
                    }],
                },
            );
            flag
        };

        let (tx, rx) = async_channel::bounded::<(Option<DecodedRgba>, Option<OwnedSemaphorePermit>)>(1);
        let cache = self.cache.clone();
        let http = self.http.clone();
        let net_sem = self.net_sem.clone();
        let decode_sem = self.decode_sem.clone();
        let ready_slots = self.ready_slots.clone();
        let compressed_lru = self.compressed_lru.clone();
        let disk_save_tx = self.disk_save_tx.clone();
        let disk_key_owned = disk_key.to_string();
        let url_owned = url.to_string();

        self.runtime.spawn(async move {
            if cancelled.load(Ordering::Relaxed) {
                let _ = tx.send((None, None)).await;
                return;
            }

            // 1. Check 5 MB in-memory compressed JPEG/WebP ring first (zero HDD seeks!)
            let mut compressed_opt: Option<Arc<[u8]>> = compressed_lru
                .lock()
                .ok()
                .and_then(|mut lru| lru.get(&disk_key_owned));

            if compressed_opt.is_some() { crate::diagnostics::image_event(crate::diagnostics::ImageEvent::CompressedHit, 1); }

            // 2. Cold miss: check disk cache or fetch over HTTP
            if compressed_opt.is_none() {
                let _permit = net_sem.acquire_owned().await.ok();
                if cancelled.load(Ordering::Relaxed) {
                    let _ = tx.send((None, None)).await;
                    return;
                }

                let disk_timer = crate::diagnostics::AggregateTimer::new(crate::diagnostics::ImageEvent::DiskTimeUs);
                let from_disk = if cache.has_thumbnail(&disk_key_owned) {
                    tokio::fs::read(cache.get_thumbnail_path(&disk_key_owned))
                        .await
                        .ok()
                        .filter(|b| !b.is_empty())
                        .map(Arc::<[u8]>::from)
                } else {
                    None
                };

                drop(disk_timer);
                if let Some(arc_bytes) = from_disk {
                    crate::diagnostics::image_event(crate::diagnostics::ImageEvent::DiskHit, 1);
                    if let Ok(mut lru) = compressed_lru.lock() {
                        lru.insert(disk_key_owned.clone(), arc_bytes.clone());
                    }
                    compressed_opt = Some(arc_bytes);
                } else {
                    crate::diagnostics::image_event(crate::diagnostics::ImageEvent::Network, 1);
                    let _network_timer = crate::diagnostics::AggregateTimer::new(crate::diagnostics::ImageEvent::NetworkTimeUs);
                    if let Ok(resp) = http.get(&url_owned).send().await {
                        if cancelled.load(Ordering::Relaxed) {
                            let _ = tx.send((None, None)).await;
                            return;
                        }
                        if let Ok(ok_resp) = resp.error_for_status() {
                            if let Ok(bytes) = ok_resp.bytes().await {
                                if !bytes.is_empty() {
                                    let arc_bytes: Arc<[u8]> = Arc::from(bytes.as_ref());
                                    if let Ok(mut lru) = compressed_lru.lock() {
                                        lru.insert(disk_key_owned.clone(), arc_bytes.clone());
                                    }
                                    let _ = disk_save_tx.try_send((disk_key_owned, arc_bytes.clone()));
                                    compressed_opt = Some(arc_bytes);
                                }
                            }
                        }
                    }
                }
            }

            if cancelled.load(Ordering::Relaxed) {
                let _ = tx.send((None, None)).await;
                return;
            }

            // 3. Acquire bounded in-flight buffer slot BEFORE unpacking RGBA so decoded `Vec<u8>`
            //    buffers can never pile up faster than the 16ms Wayland frame pacer drains them.
            let Some( bytes_arc ) = compressed_opt else {
                let _ = tx.send((None, None)).await;
                return;
            };
            let slot_permit = ready_slots.acquire_owned().await.ok();
            if cancelled.load(Ordering::Relaxed) {
                let _ = tx.send((None, None)).await;
                return;
            }

            let _dec_permit = decode_sem.acquire_owned().await.ok();
            if cancelled.load(Ordering::Relaxed) {
                let _ = tx.send((None, None)).await;
                return;
            }

            let decoded = tokio::task::spawn_blocking(move || {
                crate::diagnostics::image_event(crate::diagnostics::ImageEvent::Decode, 1);
                let _timer = crate::diagnostics::AggregateTimer::new(crate::diagnostics::ImageEvent::DecodeTimeUs);
                decode_and_scale_rgba(&bytes_arc, target)
            })
                .await
                .ok()
                .flatten();

            let _ = tx.send((decoded, slot_permit)).await;
        });

        let loader_weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (decoded_opt, slot_permit) = rx.recv().await.unwrap_or((None, None));
            let Some(loader) = loader_weak.upgrade() else {
                return;
            };
            loader
                .ready_queue
                .borrow_mut()
                .push_back((mem_key, decoded_opt, slot_permit));
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
        self.sweep_cancelled_inflight();

        for _ in 0..MAX_TEXTURES_PER_FRAME {
            let Some((mem_key, decoded_opt, _slot_permit)) =
                self.ready_queue.borrow_mut().pop_front()
            else {
                break;
            };

            let mut live_waiters: Vec<WaiterEntry> = self
                .inflight
                .borrow_mut()
                .remove(&mem_key)
                .map(|e| e.waiters)
                .unwrap_or_default();
            live_waiters.retain(|w| (w.is_alive)());

            // If every requesting card scrolled out of window before decode finished, drop the
            // RGBA buffer immediately without uploading an unused texture to integrated GPU RAM.
            if live_waiters.is_empty() {
                continue;
            }

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

            for waiter in live_waiters {
                crate::diagnostics::image_event(crate::diagnostics::ImageEvent::Delivered, 1);
                (waiter.on_done)(texture_opt.clone());
            }
        }
    }

    pub fn load_into_picture(self: &Rc<Self>, key: &str, url: &str, picture: &Picture) {
        if url.is_empty() {
            return;
        }
        let pic_alive = picture.downgrade();
        let pic_weak = picture.downgrade();
        self.load_target(
            key,
            url,
            ImageTarget::Card16x9,
            move || pic_alive.upgrade().is_some(),
            move |tex_opt| {
                if let (Some(pic), Some(tex)) = (pic_weak.upgrade(), tex_opt) {
                    pic.set_paintable(Some(&tex));
                }
            },
        );
    }

    pub fn load_into_short_picture(self: &Rc<Self>, key: &str, url: &str, picture: &Picture) {
        if url.is_empty() {
            return;
        }
        let pic_alive = picture.downgrade();
        let pic_weak = picture.downgrade();
        self.load_target(
            key,
            url,
            ImageTarget::Short9x16,
            move || pic_alive.upgrade().is_some(),
            move |tex_opt| {
                if let (Some(pic), Some(tex)) = (pic_weak.upgrade(), tex_opt) {
                    pic.set_paintable(Some(&tex));
                }
            },
        );
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
        let av_alive = avatar.downgrade();
        let av_weak = avatar.downgrade();
        self.load_target(
            &cache_key,
            url,
            ImageTarget::Avatar,
            move || av_alive.upgrade().is_some(),
            move |tex_opt| {
                if let (Some(av), Some(tex)) = (av_weak.upgrade(), tex_opt) {
                    av.set_custom_image(Some(&tex));
                }
            },
        );
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

        lru.shrink_to(1);
        assert_eq!(lru.len(), 1);
        assert_eq!(lru.get("d"), Some(4));
    }

    #[test]
    fn test_byte_budget_lru_enforces_strict_cap() {
        let mut ring = ByteBudgetLru::new(2048);
        let b600: Arc<[u8]> = Arc::from(vec![7u8; 600]);
        ring.insert("k1".to_string(), b600.clone());
        ring.insert("k2".to_string(), b600.clone());
        ring.insert("k3".to_string(), b600.clone());
        assert_eq!(ring.used_bytes(), 1800);

        // Touch k1 so k2 becomes oldest
        assert!(ring.get("k1").is_some());

        // Inserting k4 (600 bytes -> 2400 > 2048) must evict k2
        ring.insert("k4".to_string(), b600);
        assert_eq!(ring.used_bytes(), 1800);
        assert!(ring.get("k2").is_none());
        assert!(ring.get("k1").is_some());
        assert!(ring.get("k3").is_some());
        assert!(ring.get("k4").is_some());
    }

    #[test]
    fn test_off_thread_decode_and_scale() {
        let img = RgbaImage::from_pixel(640, 480, Rgba([212, 85, 42, 255]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();

        let card = decode_and_scale_rgba(buf.get_ref(), ImageTarget::Card16x9).unwrap();
        assert_eq!(card.width, 288);
        assert_eq!(card.height, 162);
        assert_eq!(card.rgba.len(), 288 * 162 * 4);

        let short = decode_and_scale_rgba(buf.get_ref(), ImageTarget::Short9x16).unwrap();
        assert_eq!(short.width, 158);
        assert_eq!(short.height, 280);
        assert_eq!(short.rgba.len(), 158 * 280 * 4);
    }
}
