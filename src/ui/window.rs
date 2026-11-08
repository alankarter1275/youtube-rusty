use crate::backend::cache::Cache;
use crate::backend::engine::Engine;
use crate::backend::models::VideoSummary;
use crate::player::mpv::{self, Quality};
use crate::ui::details_sheet::{ContextMenu, DetailsSheet};
use crate::ui::rail::{Rail, RailTab};
use crate::ui::video_card::VideoCard;
use adw::prelude::*;
use gtk4::gdk::{Display, Texture};
use gtk4::gio;
use gtk4::{
    Align, Box as GtkBox, Button, CssProvider, FlowBox, Label, MenuButton, Orientation, Overlay,
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
    pub section_title: Label,
    pub toast_overlay: adw::ToastOverlay,
    engine: Arc<Engine>,
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        load_custom_css();

        let style_manager = adw::StyleManager::default();
        style_manager.set_color_scheme(adw::ColorScheme::PreferDark);

        // Header Bar (borderless, blends into #100C0A)
        let header_bar = adw::HeaderBar::new();
        header_bar.add_css_class("flat");

        let search_entry = SearchEntry::builder()
            .placeholder_text("Search YouTube...")
            .build();
        search_entry.add_css_class("yr-search");

        let search_clamp = adw::Clamp::builder()
            .maximum_size(420)
            .child(&search_entry)
            .build();
        header_bar.set_title_widget(Some(&search_clamp));

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

        // 68px Icon Rail
        let rail = Rc::new(Rail::new());

        // Navigation View
        let nav_view = adw::NavigationView::new();

        // Home View Container: [ Chips Row ] + [ Section Title: Playfair 18 ] + [ FlowBox Grid ]
        let home_vbox = GtkBox::new(Orientation::Vertical, 16);
        home_vbox.set_margin_top(18);
        home_vbox.set_margin_start(24);
        home_vbox.set_margin_end(24);
        home_vbox.set_margin_bottom(24);

        // Chips Row (Trending, Gaming, Music, News)
        let chips_box = GtkBox::new(Orientation::Horizontal, 8);
        chips_box.set_halign(Align::Start);
        let chip_trending = create_chip("Trending", true);
        let chip_gaming = create_chip("Gaming", false);
        let chip_music = create_chip("Music", false);
        let chip_news = create_chip("News", false);

        chips_box.append(&chip_trending);
        chips_box.append(&chip_gaming);
        chips_box.append(&chip_music);
        chips_box.append(&chip_news);

        // Section Title: Playfair 18
        let section_title = Label::new(Some("Trending"));
        section_title.set_halign(Align::Start);
        section_title.add_css_class("yr-h2");
        section_title.set_margin_top(6);

        // Video Grid (4 columns at reference width)
        let flow_box = FlowBox::builder()
            .selection_mode(SelectionMode::None)
            .homogeneous(false)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .column_spacing(20)
            .row_spacing(24)
            .build();

        home_vbox.append(&chips_box);
        home_vbox.append(&section_title);
        home_vbox.append(&flow_box);

        let content_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&home_vbox)
            .build();

        let home_page = adw::NavigationPage::builder()
            .title("Home")
            .tag("home")
            .child(&content_scrolled)
            .build();

        nav_view.push(&home_page);

        // Main Horizontal Layout: Rail + Navigation View
        let main_h_box = GtkBox::new(Orientation::Horizontal, 0);
        main_h_box.append(&rail.container);
        main_h_box.append(&nav_view);

        let content_overlay = Overlay::new();
        content_overlay.set_child(Some(&main_h_box));

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header_bar);
        toolbar_view.set_content(Some(&content_overlay));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

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
            section_title,
            toast_overlay,
            engine,
            cache,
            runtime,
        });

        this.setup_events(&chip_trending, &chip_gaming, &chip_music, &chip_news);
        this.load_trending();

        this
    }

    fn setup_events(
        self: &Rc<Self>,
        chip_trending: &Button,
        chip_gaming: &Button,
        chip_music: &Button,
        chip_news: &Button,
    ) {
        // Search bar activation -> Search Results Mode
        let this = self.clone();
        self.search_entry.connect_activate(move |entry| {
            let text = entry.text().to_string();
            if !text.trim().is_empty() {
                this.search(&text);
            }
        });

        // Rail Navigation Buttons
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

        // Category Chips
        let this = self.clone();
        let ct = chip_trending.clone();
        let cg = chip_gaming.clone();
        let cm = chip_music.clone();
        let cn = chip_news.clone();
        chip_trending.connect_clicked(move |_| {
            reset_chips(&[&ct, &cg, &cm, &cn], &ct);
            this.load_trending();
        });

        let this = self.clone();
        let ct = chip_trending.clone();
        let cg = chip_gaming.clone();
        let cm = chip_music.clone();
        let cn = chip_news.clone();
        chip_gaming.connect_clicked(move |_| {
            reset_chips(&[&ct, &cg, &cm, &cn], &cg);
            this.search_category("Gaming");
        });

        let this = self.clone();
        let ct = chip_trending.clone();
        let cg = chip_gaming.clone();
        let cm = chip_music.clone();
        let cn = chip_news.clone();
        chip_music.connect_clicked(move |_| {
            reset_chips(&[&ct, &cg, &cm, &cn], &cm);
            this.search_category("Music");
        });

        let this = self.clone();
        let ct = chip_trending.clone();
        let cg = chip_gaming.clone();
        let cm = chip_music.clone();
        let cn = chip_news.clone();
        chip_news.connect_clicked(move |_| {
            reset_chips(&[&ct, &cg, &cm, &cn], &cn);
            this.search_category("News");
        });

        // FlowBox card activation -> Details Sheet (Phase 3)
        let this = self.clone();
        self.flow_box.connect_child_activated(move |_, child| {
            if let Some(summary_ref) = unsafe { child.data::<VideoSummary>("video_summary") } {
                let summary = unsafe { summary_ref.as_ref() }.clone();
                let thumb_texture =
                    unsafe { child.data::<Texture>("thumb_texture") }.map(|t| unsafe { t.as_ref() });
                let avatar_texture =
                    unsafe { child.data::<Texture>("avatar_texture") }.map(|t| unsafe { t.as_ref() });

                let toast_overlay = this.toast_overlay.clone();
                let on_toast = move |msg: String| {
                    toast_overlay.add_toast(adw::Toast::new(&msg));
                };

                let toast_overlay_play = this.toast_overlay.clone();
                let on_play = move |id: String, title: String, quality: Quality| {
                    let q_name = match quality {
                        Quality::P360 => "360p",
                        Quality::P720 => "720p",
                        Quality::P1080 => "1080p",
                        Quality::AudioOnly => "Audio",
                    };
                    toast_overlay_play.add_toast(adw::Toast::new(&format!(
                        "Playing \"{title}\" ({q_name}) in MPV..."
                    )));
                    let _ = mpv::play_with_quality(&id, &title, quality);
                };

                let on_open_channel = move |_cid: String, _cname: String| {
                    // Phase 5: channel page
                };

                DetailsSheet::show(
                    &this.window,
                    &summary,
                    thumb_texture,
                    avatar_texture,
                    on_play,
                    on_toast,
                    on_open_channel,
                );
            }
        });
    }

    pub fn search(self: &Rc<Self>, query: &str) {
        self.section_title
            .set_text(&format!("Results for \"{query}\""));
        self.show_skeletons(6);

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
                    Ok(videos) => {
                        if videos.is_empty() {
                            this.show_empty_state("No videos found", "Try different keywords or check spelling");
                        } else {
                            this.populate_videos(videos, ViewMode::SearchList);
                        }
                    }
                    Err(e) => {
                        this.show_empty_state("Failed to load results", &e);
                    }
                }
            }
        });
    }

    pub fn search_category(self: &Rc<Self>, category: &str) {
        self.section_title.set_text(category);
        self.show_skeletons(8);

        let (sender, receiver) = async_channel::unbounded::<Result<Vec<VideoSummary>, String>>();
        let engine = self.engine.clone();
        let cat = category.to_string();

        self.runtime.spawn(async move {
            let res = engine.search(&cat).await;
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

    pub fn load_trending(self: &Rc<Self>) {
        self.section_title.set_text("Trending");
        self.show_skeletons(8);

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

    fn show_skeletons(self: &Rc<Self>, count: usize) {
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_skeleton());
        }
    }

    fn show_empty_state(self: &Rc<Self>, title: &str, description: &str) {
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        let status_page = adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title(title)
            .description(description)
            .build();

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&status_page));
        self.flow_box.append(&container);
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
                card.container
                    .set_data("video_summary", summary.clone());
            }

            let toast_overlay = self.toast_overlay.clone();
            let on_toast = move |msg: String| {
                toast_overlay.add_toast(adw::Toast::new(&msg));
            };

            let toast_overlay_play = self.toast_overlay.clone();
            let on_play = move |id: String, title: String, quality: Quality| {
                let q_name = match quality {
                    Quality::P360 => "360p",
                    Quality::P720 => "720p",
                    Quality::P1080 => "1080p",
                    Quality::AudioOnly => "Audio",
                };
                toast_overlay_play.add_toast(adw::Toast::new(&format!(
                    "Playing \"{title}\" ({q_name}) in MPV..."
                )));
                let _ = mpv::play_with_quality(&id, &title, quality);
            };

            let on_open_channel = move |_cid: String, _cname: String| {
                // Phase 5
            };

            ContextMenu::attach(
                &card.container,
                &summary,
                on_play,
                on_toast,
                on_open_channel,
            );

            self.load_thumbnail_async(&card, &summary.thumbnail_url, &summary.id);
            if !summary.channel_avatar_url.is_empty() {
                self.load_avatar_async(&card, &summary.channel_avatar_url, &summary.channel_id);
            }

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
        let container = card.container.clone();
        glib::spawn_future_local(async move {
            if let Ok(Some(bytes)) = receiver.recv().await {
                let glib_bytes = glib::Bytes::from(&bytes);
                if let Ok(texture) = Texture::from_bytes(&glib_bytes) {
                    picture.set_paintable(Some(&texture));
                    unsafe {
                        container.set_data("thumb_texture", texture);
                    }
                }
            }
        });
    }

    fn load_avatar_async(self: &Rc<Self>, card: &VideoCard, url: &str, channel_id: &str) {
        if url.is_empty() {
            return;
        }

        let (sender, receiver) = async_channel::unbounded::<Option<Vec<u8>>>();
        let cache = self.cache.clone();
        let u = url.to_string();
        let cid = format!("avatar_{channel_id}");

        self.runtime.spawn(async move {
            if cache.has_thumbnail(&cid) {
                if let Ok(bytes) = std::fs::read(cache.get_thumbnail_path(&cid)) {
                    let _ = sender.send(Some(bytes)).await;
                    return;
                }
            }

            if let Ok(resp) = reqwest::get(&u).await {
                if let Ok(bytes) = resp.bytes().await {
                    let vec_bytes = bytes.to_vec();
                    let _ = cache.save_thumbnail(&cid, &vec_bytes);
                    let _ = sender.send(Some(vec_bytes)).await;
                    return;
                }
            }

            let _ = sender.send(None).await;
        });

        let avatar = card.avatar.clone();
        let container = card.container.clone();
        glib::spawn_future_local(async move {
            if let Ok(Some(bytes)) = receiver.recv().await {
                let glib_bytes = glib::Bytes::from(&bytes);
                if let Ok(texture) = Texture::from_bytes(&glib_bytes) {
                    avatar.set_custom_image(Some(&texture));
                    unsafe {
                        container.set_data("avatar_texture", texture);
                    }
                }
            }
        });
    }

    pub fn present(&self) {
        self.window.present();
    }
}

fn create_chip(label: &str, is_active: bool) -> Button {
    let btn = Button::builder().label(label).build();
    btn.add_css_class("yr-chip");
    if is_active {
        btn.add_css_class("active");
    }
    btn
}

fn reset_chips(chips: &[&Button], active_chip: &Button) {
    for chip in chips {
        chip.remove_css_class("active");
    }
    active_chip.add_css_class("active");
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
