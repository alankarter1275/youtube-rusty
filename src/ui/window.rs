use crate::backend::cache::Cache;
use crate::backend::engine::Engine;
use crate::backend::models::VideoSummary;
use crate::player::mpv;
use crate::ui::video_card::VideoCard;
use adw::prelude::*;
use gtk4::gdk::{Display, Texture};
use gtk4::{
    Box as GtkBox, CssProvider, FlowBox, Label, ListBox, ListBoxRow, Orientation, ScrolledWindow,
    SearchEntry, SelectionMode, StyleContext,
};
use libadwaita as adw;
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Runtime;

#[derive(Clone, Copy, PartialEq)]
pub enum ViewMode {
    Grid,
    SearchList,
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub search_entry: SearchEntry,
    pub flow_box: FlowBox,
    pub toast_overlay: adw::ToastOverlay,
    pub sidebar_list: ListBox,
    engine: Arc<Engine>,
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        // Load custom styling
        load_custom_css();

        let style_manager = adw::StyleManager::default();
        style_manager.set_color_scheme(adw::ColorScheme::PreferDark);

        // Header bar
        let header_bar = adw::HeaderBar::new();
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search YouTube...")
            .width_request(460)
            .build();
        header_bar.set_title_widget(Some(&search_entry));

        // Left Sidebar (Navigation drawer)
        let sidebar_list = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        sidebar_list.add_css_class("sidebar-list");

        sidebar_list.append(&create_sidebar_row("🔥 Trending", "trending"));
        sidebar_list.append(&create_sidebar_row("🎮 Gaming", "gaming"));
        sidebar_list.append(&create_sidebar_row("🎵 Music", "music"));
        sidebar_list.append(&create_sidebar_row("📰 News", "news"));

        let sidebar_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .width_request(190)
            .child(&sidebar_list)
            .build();

        // Main content area: video flow box inside scrolled window
        let flow_box = FlowBox::builder()
            .selection_mode(SelectionMode::None)
            .homogeneous(false)
            .max_children_per_line(5)
            .min_children_per_line(1)
            .column_spacing(18)
            .row_spacing(20)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let content_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&flow_box)
            .build();

        // Horizontal split container (Sidebar + Content)
        let main_h_box = GtkBox::new(Orientation::Horizontal, 0);
        main_h_box.append(&sidebar_scrolled);
        main_h_box.append(&content_scrolled);

        let content_box = GtkBox::new(Orientation::Vertical, 0);
        content_box.append(&header_bar);
        content_box.append(&main_h_box);

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&content_box));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("YouTube Rusty")
            .default_width(1280)
            .default_height(780)
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
            sidebar_list,
            engine,
            cache,
            runtime,
        });

        this.setup_events();
        this.load_trending();

        this
    }

    fn setup_events(self: &Rc<Self>) {
        // Search bar activation -> Search List View
        let this = self.clone();
        self.search_entry.connect_activate(move |entry| {
            let text = entry.text().to_string();
            if !text.trim().is_empty() {
                this.search(&text);
            }
        });

        // Sidebar row selection -> Grid View
        let this = self.clone();
        self.sidebar_list.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                let tag = unsafe { r.data::<String>("tag") };
                if let Some(tag_str) = tag {
                    let s = unsafe { tag_str.as_ref() };
                    if s == "trending" {
                        this.load_trending();
                    } else {
                        this.search(s);
                    }
                }
            }
        });

        // Video card click -> Stream in MPV
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
                    Ok(videos) => this.populate_videos(videos, ViewMode::SearchList),
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
                    this.populate_videos(videos, ViewMode::Grid);
                }
            }
        });
    }

    fn populate_videos(self: &Rc<Self>, videos: Vec<VideoSummary>, mode: ViewMode) {
        // Clear previous video cards
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        // Adjust layout flow rules
        match mode {
            ViewMode::Grid => {
                self.flow_box.set_max_children_per_line(5);
                self.flow_box.set_min_children_per_line(1);
                self.flow_box.set_column_spacing(18);
                self.flow_box.set_row_spacing(20);
            }
            ViewMode::SearchList => {
                // In search list mode: 1 item per line, vertical stack of horizontal cards
                self.flow_box.set_max_children_per_line(1);
                self.flow_box.set_min_children_per_line(1);
                self.flow_box.set_column_spacing(0);
                self.flow_box.set_row_spacing(12);
            }
        }

        for summary in videos {
            let card = match mode {
                ViewMode::Grid => VideoCard::new_grid(&summary),
                ViewMode::SearchList => VideoCard::new_search(&summary),
            };

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

fn create_sidebar_row(title: &str, tag: &str) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.add_css_class("sidebar-row");
    let label = Label::new(Some(title));
    label.set_halign(gtk4::Align::Start);
    label.add_css_class("heading");
    row.set_child(Some(&label));
    unsafe {
        row.set_data("tag", tag.to_string());
    }
    row
}

fn load_custom_css() {
    let provider = CssProvider::new();
    let css = include_str!("style.css");
    provider.load_from_data(css);

    if let Some(display) = Display::default() {
        StyleContext::add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
