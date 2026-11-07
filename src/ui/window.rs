use crate::backend::cache::Cache;
use crate::backend::engine::Engine;
use crate::backend::models::VideoSummary;
use crate::player::mpv;
use crate::ui::video_card::VideoCard;
use adw::prelude::*;
use gtk4::gdk::Texture;
use gtk4::{Box as GtkBox, FlowBox, Orientation, ScrolledWindow, SearchEntry, SelectionMode};
use libadwaita as adw;
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Runtime;

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub search_entry: SearchEntry,
    pub flow_box: FlowBox,
    pub toast_overlay: adw::ToastOverlay,
    engine: Arc<Engine>,
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let style_manager = adw::StyleManager::default();
        style_manager.set_color_scheme(adw::ColorScheme::PreferDark);

        let header_bar = adw::HeaderBar::new();
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search YouTube...")
            .width_request(420)
            .build();
        header_bar.set_title_widget(Some(&search_entry));

        let flow_box = FlowBox::builder()
            .selection_mode(SelectionMode::None)
            .homogeneous(false)
            .max_children_per_line(6)
            .min_children_per_line(1)
            .column_spacing(16)
            .row_spacing(16)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();

        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&flow_box)
            .build();

        let content_box = GtkBox::new(Orientation::Vertical, 0);
        content_box.append(&header_bar);
        content_box.append(&scrolled_window);

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&content_box));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("YouTube Rusty")
            .default_width(1180)
            .default_height(750)
            .content(&toast_overlay)
            .build();

        let engine = Arc::new(Engine::new());
        let cache = Arc::new(Cache::new());
        let runtime = Arc::new(Runtime::new().expect("Failed to initialize Tokio runtime"));

        let this = Rc::new(Self {
            window,
            search_entry,
            flow_box,
            toast_overlay,
            engine,
            cache,
            runtime,
        });

        this.setup_events();
        this.load_trending();

        this
    }

    fn setup_events(self: &Rc<Self>) {
        // Search activation
        let this = self.clone();
        self.search_entry.connect_activate(move |entry| {
            let text = entry.text().to_string();
            if !text.trim().is_empty() {
                this.search(&text);
            }
        });

        // Click / Activate video child
        let this = self.clone();
        self.flow_box.connect_child_activated(move |_, child| {
            if let Some(id) = unsafe { child.data::<String>("video_id") } {
                let id_str = unsafe { id.as_ref() };
                let title = unsafe {
                    child
                        .data::<String>("video_title")
                        .map(|t| t.as_ref().clone())
                        .unwrap_or_else(|| "YouTube Video".to_string())
                };

                let toast = adw::Toast::new(&format!("Playing \"{title}\" in MPV..."));
                this.toast_overlay.add_toast(toast);

                let _ = mpv::play(id_str, &title);
            }
        });
    }

    pub fn search(self: &Rc<Self>, query: &str) {
        let (sender, receiver) = async_channel::unbounded::<Result<Vec<VideoSummary>, String>>();
        let engine = self.engine.clone();
        let q = query.to_string();

        self.runtime.spawn(async move {
            let res = engine.search(&q).await;
            let _ = sender.send(res).await;
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(res) = receiver.recv().await {
                match res {
                    Ok(videos) => this.populate_videos(videos),
                    Err(e) => {
                        let toast = adw::Toast::new(&format!("Error: {e}"));
                        this.toast_overlay.add_toast(toast);
                    }
                }
            }
        });
    }

    pub fn load_trending(self: &Rc<Self>) {
        let (sender, receiver) = async_channel::unbounded::<Result<Vec<VideoSummary>, String>>();
        let engine = self.engine.clone();

        self.runtime.spawn(async move {
            let res = engine.trending().await;
            let _ = sender.send(res).await;
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(res) = receiver.recv().await {
                if let Ok(videos) = res {
                    this.populate_videos(videos);
                }
            }
        });
    }

    fn populate_videos(self: &Rc<Self>, videos: Vec<VideoSummary>) {
        // Clear old items
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        for summary in videos {
            let card = VideoCard::new(&summary);

            // Store metadata on FlowBoxChild for activation handler
            unsafe {
                card.container.set_data("video_id", summary.id.clone());
                card.container.set_data("video_title", summary.title.clone());
            }

            self.load_thumbnail_async(&card, &summary.thumbnail_url, &summary.id);
            self.flow_box.append(&card.container);
        }
    }

    fn load_thumbnail_async(self: &Rc<Self>, card: &VideoCard, url: &str, id: &str) {
        if url.is_empty() {
            return;
        }

        let (sender, receiver) = async_channel::unbounded::<Option<Vec<u8>>>();
        let cache = self.cache.clone();
        let u = url.to_string();
        let vid = id.to_string();

        self.runtime.spawn(async move {
            if cache.has_thumbnail(&vid) {
                if let Ok(bytes) = std::fs::read(cache.get_thumbnail_path(&vid)) {
                    let _ = sender.send(Some(bytes)).await;
                    return;
                }
            }

            if let Ok(resp) = reqwest::get(&u).await {
                if let Ok(bytes) = resp.bytes().await {
                    let vec_bytes = bytes.to_vec();
                    let _ = cache.save_thumbnail(&vid, &vec_bytes);
                    let _ = sender.send(Some(vec_bytes)).await;
                    return;
                }
            }

            let _ = sender.send(None).await;
        });

        let picture = card.picture.clone();
        glib::spawn_future_local(async move {
            if let Ok(Some(bytes)) = receiver.recv().await {
                let glib_bytes = glib::Bytes::from(&bytes);
                if let Ok(texture) = Texture::from_bytes(&glib_bytes) {
                    picture.set_paintable(Some(&texture));
                }
            }
        });
    }

    pub fn present(&self) {
        self.window.present();
    }
}
