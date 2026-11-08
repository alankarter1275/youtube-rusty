use crate::backend::cache::Cache;
use adw::prelude::*;
use gtk4::gdk::Texture;
use gtk4::Picture;
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;

type Waiter = Box<dyn FnOnce(Option<Texture>)>;

pub struct ImageLoader {
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
    http: reqwest::Client,
    semaphore: Arc<Semaphore>,
    textures: RefCell<HashMap<String, Texture>>,
    inflight: RefCell<HashMap<String, Vec<Waiter>>>,
}

impl ImageLoader {
    pub fn new(cache: Arc<Cache>, runtime: Arc<Runtime>) -> Rc<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Rc::new(Self {
            cache,
            runtime,
            http,
            semaphore: Arc::new(Semaphore::new(8)),
            textures: RefCell::new(HashMap::new()),
            inflight: RefCell::new(HashMap::new()),
        })
    }

    pub fn get_cached(&self, key: &str) -> Option<Texture> {
        self.textures.borrow().get(key).cloned()
    }

    pub fn clear_all(&self) {
        self.textures.borrow_mut().clear();
        self.cache.clear_all();
    }

    pub fn load(self: &Rc<Self>, key: &str, url: &str, on_done: impl FnOnce(Option<Texture>) + 'static) {
        if key.is_empty() || url.is_empty() {
            on_done(None);
            return;
        }

        if let Some(tex) = self.get_cached(key) {
            on_done(Some(tex));
            return;
        }

        {
            let mut inflight = self.inflight.borrow_mut();
            if let Some(waiters) = inflight.get_mut(key) {
                waiters.push(Box::new(on_done));
                return;
            }
            inflight.insert(key.to_string(), vec![Box::new(on_done)]);
        }

        let (tx, rx) = async_channel::bounded::<Option<Vec<u8>>>(1);
        let cache = self.cache.clone();
        let http = self.http.clone();
        let sem = self.semaphore.clone();
        let key_owned = key.to_string();
        let url_owned = url.to_string();

        self.runtime.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();

            if cache.has_thumbnail(&key_owned) {
                if let Ok(bytes) = std::fs::read(cache.get_thumbnail_path(&key_owned)) {
                    if !bytes.is_empty() {
                        let _ = tx.send(Some(bytes)).await;
                        return;
                    }
                }
            }

            if let Ok(resp) = http.get(&url_owned).send().await {
                if let Ok(ok_resp) = resp.error_for_status() {
                    if let Ok(bytes) = ok_resp.bytes().await {
                        if !bytes.is_empty() {
                            let vec_bytes = bytes.to_vec();
                            let _ = cache.save_thumbnail(&key_owned, &vec_bytes);
                            let _ = tx.send(Some(vec_bytes)).await;
                            return;
                        }
                    }
                }
            }

            let _ = tx.send(None).await;
        });

        let loader_weak = Rc::downgrade(self);
        let key_ui = key.to_string();
        glib::spawn_future_local(async move {
            let bytes_opt = rx.recv().await.ok().flatten();
            let Some(loader) = loader_weak.upgrade() else {
                return;
            };

            let texture_opt = bytes_opt.and_then(|bytes| {
                let glib_bytes = glib::Bytes::from(&bytes);
                Texture::from_bytes(&glib_bytes).ok()
            });

            if let Some(ref tex) = texture_opt {
                let mut map = loader.textures.borrow_mut();
                if map.len() >= 300 {
                    map.clear();
                }
                map.insert(key_ui.clone(), tex.clone());
            }

            let waiters = loader
                .inflight
                .borrow_mut()
                .remove(&key_ui)
                .unwrap_or_default();
            for waiter in waiters {
                waiter(texture_opt.clone());
            }
        });
    }

    pub fn load_into_picture(self: &Rc<Self>, key: &str, url: &str, picture: &Picture) {
        if url.is_empty() {
            return;
        }
        let pic_weak = picture.downgrade();
        self.load(key, url, move |tex_opt| {
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
            format!("avatar_url_{}", crate::backend::cache::sanitize_cache_key(url))
        };
        let av_weak = avatar.downgrade();
        self.load(&cache_key, url, move |tex_opt| {
            if let (Some(av), Some(tex)) = (av_weak.upgrade(), tex_opt) {
                av.set_custom_image(Some(&tex));
            }
        });
    }
}
