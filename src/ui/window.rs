use crate::backend::cache::Cache;
use crate::backend::engine::Engine;
use crate::backend::models::VideoSummary;
use crate::player::mpv;
use crate::ui::rail::{Rail, RailTab};
use crate::ui::video_card::VideoCard;
use adw::prelude::*;
use gtk4::gdk::{Display, Texture};
use gtk4::gio;
use gtk4::{
    Box as GtkBox, CssProvider, FlowBox, MenuButton, Orientation, Overlay,
    ScrolledWindow, SearchEntry, SelectionMode,
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
    pub toolbar_view: adw::ToolbarView,
    pub header_bar: adw::HeaderBar,
    pub search_entry: SearchEntry,
    pub rail: Rc<Rail>,
    pub nav_view: adw::NavigationView,
    pub flow_box: FlowBox,
    pub toast_overlay: adw::ToastOverlay,
    engine: Arc<Engine>,
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        load_custom_css();

        // Enforce dark style matching Kalam Terracotta theme
        let style_manager = adw::StyleManager::default();
        style_manager.set_color_scheme(adw::ColorScheme::PreferDark);

        // Header Bar (borderless, blends with #100C0A)
        let header_bar = adw::HeaderBar::new();
        header_bar.add_css_class("flat");

        // Centered Search Entry wrapped in AdwClamp (max 420px)
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search YouTube...")
            .build();
        search_entry.add_css_class("yr-search");

        let search_clamp = adw::Clamp::builder()
            .maximum_size(420)
            .child(&search_entry)
            .build();
        header_bar.set_title_widget(Some(&search_clamp));

        // Primary Menu Button at the right (Preferences, Shortcuts, About)
        let menu_model = gio::Menu::new();
        menu_model.append(Some("Preferences"), Some("app.preferences"));
        menu_model.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        menu_model.append(Some("About YouTube Rusty"), Some("app.about"));

        let primary_menu_btn = MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu_model)
            .tooltip_text("Main Menu")
            .build();
        primary_menu_btn.add_css_class("yr-rail-btn");
        header_bar.pack_end(&primary_menu_btn);

        // Left 68px Icon Rail
        let rail = Rc::new(Rail::new());

        // Navigation View for Content (Enables pushing Channel / Search pages with back support)
        let nav_view = adw::NavigationView::new();

        // Home Page content: FlowBox inside smooth ScrolledWindow
        let flow_box = FlowBox::builder()
            .selection_mode(SelectionMode::None)
            .homogeneous(false)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .column_spacing(20)
            .row_spacing(24)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(24)
            .margin_end(24)
            .build();

        let content_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&flow_box)
            .build();

        let home_page = adw::NavigationPage::builder()
            .title("Home")
            .tag("home")
            .child(&content_scrolled)
            .build();

        nav_view.push(&home_page);

        // Horizontal Layout: Rail + Navigation View
        let main_h_box = GtkBox::new(Orientation::Horizontal, 0);
        main_h_box.append(&rail.container);
        main_h_box.append(&nav_view);

        // Overlay over content area (for persistent floating Player Card in future phase)
        let content_overlay = Overlay::new();
        content_overlay.set_child(Some(&main_h_box));

        // Toolbar View managing top bar & content
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header_bar);
        toolbar_view.set_content(Some(&content_overlay));

        // Toast overlay at root
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        // Main Window
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("youtube-rusty")
            .default_width(1280)
            .default_height(780)
            .content(&toast_overlay)
            .build();

        let engine = Arc::new(Engine::new());
        let cache = Arc::new(Cache::new());
        let runtime = Arc::new(Runtime::new().expect("Failed to initialize Tokio runtime"));

        let this = Rc::new(Self {
            window,
            toolbar_view,
            header_bar,
            search_entry,
            rail,
            nav_view,
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
        // Search bar activation
        let this = self.clone();
        self.search_entry.connect_activate(move |entry| {
            let text = entry.text().to_string();
            if !text.trim().is_empty() {
                this.search(&text);
            }
        });

        // Rail Buttons
        let this = self.clone();
        self.rail.home_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::Home);
            this.load_trending();
        });

        let this = self.clone();
        self.rail.subs_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::Subscriptions);
            let toast = adw::Toast::new("Subscriptions (Coming in Phase 6)");
            this.toast_overlay.add_toast(toast);
        });

        let this = self.clone();
        self.rail.hist_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::History);
            let toast = adw::Toast::new("History (Coming in Phase 6)");
            this.toast_overlay.add_toast(toast);
        });

        let this = self.clone();
        self.rail.settings_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::Settings);
            let toast = adw::Toast::new("Settings (Coming in Phase 6)");
            this.toast_overlay.add_toast(toast);
        });

        // FlowBox card activation -> MPV streaming
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
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        match mode {
            ViewMode::Grid => {
                self.flow_box.set_max_children_per_line(4);
                self.flow_box.set_min_children_per_line(1);
                self.flow_box.set_column_spacing(20);
                self.flow_box.set_row_spacing(24);
            }
            ViewMode::SearchList => {
                self.flow_box.set_max_children_per_line(1);
                self.flow_box.set_min_children_per_line(1);
                self.flow_box.set_column_spacing(0);
                self.flow_box.set_row_spacing(14);
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

fn load_custom_css() {
    let provider = CssProvider::new();
    let css = include_str!("style.css");
    provider.load_from_data(css);

    if let Some(display) = Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
