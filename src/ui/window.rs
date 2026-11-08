use crate::backend::cache::Cache;
use crate::backend::engine::Engine;
use crate::backend::history::HistoryStore;
use crate::backend::models::VideoSummary;
use crate::backend::settings::SettingsStore;
use crate::backend::subscriptions::SubscriptionStore;
use crate::player::mpv::Quality;
use crate::player::mpv_ipc::{MpvEvent, MpvSession};
use crate::ui::channel_page::ChannelPage;
use crate::ui::details_sheet::{ContextMenu, DetailsSheet};
use crate::ui::player_card::PlayerCard;
use crate::ui::rail::{Rail, RailTab};
use crate::ui::video_card::VideoCard;
use adw::prelude::*;
use gtk4::gdk::{Display, Texture};
use gtk4::gio;
use gtk4::{
    Align, Box as GtkBox, Button, CssProvider, FlowBox, Label, MenuButton, Orientation, Overlay,
    Picture, ScrolledWindow, SearchEntry, SelectionMode,
};
use libadwaita as adw;
use std::cell::RefCell;
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
    pub rail: Rail,
    pub nav_view: adw::NavigationView,
    pub home_page: adw::NavigationPage,
    pub flow_box: FlowBox,
    pub section_title: Label,
    pub chips_box: GtkBox,
    pub subs_strip_box: GtkBox,
    pub subs_avatars_row: GtkBox,
    pub history_header_box: GtkBox,
    pub toast_overlay: adw::ToastOverlay,
    pub player_card: Rc<PlayerCard>,
    pub mpv_session: RefCell<Option<Arc<MpvSession>>>,
    pub mpv_pos: RefCell<f64>,
    pub mpv_duration: RefCell<f64>,
    pub history_store: Arc<HistoryStore>,
    pub settings_store: Arc<SettingsStore>,
    pub subscription_store: Arc<SubscriptionStore>,
    engine: Arc<Engine>,
    cache: Arc<Cache>,
    runtime: Arc<Runtime>,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        load_custom_css();

        let style_manager = adw::StyleManager::default();
        style_manager.set_color_scheme(adw::ColorScheme::PreferDark);

        // HeaderBar: borderless, blends into background
        let header_bar = adw::HeaderBar::new();
        header_bar.set_show_title(false);

        // Centered Search Bar: max 420px
        let search_clamp = adw::Clamp::builder().maximum_size(420).build();
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search YouTube...")
            .hexpand(true)
            .build();
        search_entry.add_css_class("yr-search");
        search_clamp.set_child(Some(&search_entry));
        header_bar.set_title_widget(Some(&search_clamp));

        // Primary Menu Button at the top right
        let menu_btn = MenuButton::new();
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.add_css_class("flat");

        let menu = gio::Menu::new();
        menu.append(Some("Settings"), Some("app.settings"));
        menu.append(Some("About youtube-rusty"), Some("app.about"));
        menu_btn.set_menu_model(Some(&menu));
        header_bar.pack_end(&menu_btn);

        // Left Navigation Rail (68px)
        let rail = Rail::new();

        // Chips Row
        let chips_box = GtkBox::new(Orientation::Horizontal, 8);
        chips_box.set_margin_start(28);
        chips_box.set_margin_end(28);
        chips_box.set_margin_top(16);
        chips_box.set_margin_bottom(12);

        let chip_trending = create_chip("Trending", true);
        let chip_gaming = create_chip("Gaming", false);
        let chip_music = create_chip("Music", false);
        let chip_news = create_chip("News", false);

        chips_box.append(&chip_trending);
        chips_box.append(&chip_gaming);
        chips_box.append(&chip_music);
        chips_box.append(&chip_news);

        // Subscriptions Avatar Strip
        let subs_strip_box = GtkBox::new(Orientation::Vertical, 8);
        subs_strip_box.set_margin_start(28);
        subs_strip_box.set_margin_end(28);
        subs_strip_box.set_margin_top(16);
        subs_strip_box.set_margin_bottom(12);
        subs_strip_box.set_visible(false);

        let subs_title = Label::new(Some("Subscriptions"));
        subs_title.set_halign(Align::Start);
        subs_title.set_xalign(0.0);
        subs_title.add_css_class("yr-h1");
        subs_strip_box.append(&subs_title);

        let subs_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .build();
        let subs_avatars_row = GtkBox::new(Orientation::Horizontal, 16);
        subs_avatars_row.set_margin_top(4);
        subs_avatars_row.set_margin_bottom(8);
        subs_scroll.set_child(Some(&subs_avatars_row));
        subs_strip_box.append(&subs_scroll);

        // History Header Box (Title + Clear button)
        let history_header_box = GtkBox::new(Orientation::Horizontal, 12);
        history_header_box.set_margin_start(28);
        history_header_box.set_margin_end(28);
        history_header_box.set_margin_top(16);
        history_header_box.set_margin_bottom(16);
        history_header_box.set_visible(false);

        let hist_title = Label::new(Some("Watch History"));
        hist_title.set_halign(Align::Start);
        hist_title.set_xalign(0.0);
        hist_title.set_hexpand(true);
        hist_title.add_css_class("yr-h1");

        let clear_hist_btn = Button::with_label("Clear history");
        clear_hist_btn.add_css_class("yr-chip");
        clear_hist_btn.set_valign(Align::Center);

        history_header_box.append(&hist_title);
        history_header_box.append(&clear_hist_btn);

        // Section Title: "Trending" (Playfair 18 / yr-h2)
        let section_title = Label::new(Some("Trending"));
        section_title.set_halign(Align::Start);
        section_title.set_xalign(0.0);
        section_title.set_margin_start(28);
        section_title.set_margin_end(28);
        section_title.set_margin_bottom(16);
        section_title.add_css_class("yr-h2");

        // Adaptive Video Grid
        let flow_box = FlowBox::new();
        flow_box.set_valign(Align::Start);
        flow_box.set_max_children_per_line(4);
        flow_box.set_min_children_per_line(1);
        flow_box.set_selection_mode(SelectionMode::None);
        flow_box.set_column_spacing(20);
        flow_box.set_row_spacing(24);
        flow_box.set_homogeneous(false);
        flow_box.set_margin_start(28);
        flow_box.set_margin_end(28);
        flow_box.set_margin_bottom(32);

        let content_v_box = GtkBox::new(Orientation::Vertical, 0);
        content_v_box.append(&chips_box);
        content_v_box.append(&subs_strip_box);
        content_v_box.append(&history_header_box);
        content_v_box.append(&section_title);
        content_v_box.append(&flow_box);

        let content_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&content_v_box)
            .vexpand(true)
            .hexpand(true)
            .build();

        // Main Navigation View
        let nav_view = adw::NavigationView::new();

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

        // Floating Player Card (Phase 4)
        let player_card = PlayerCard::new();
        content_overlay.add_overlay(&player_card.root);

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
        let history_store = Arc::new(HistoryStore::new());
        let settings_store = Arc::new(SettingsStore::new());
        let subscription_store = Arc::new(SubscriptionStore::new());

        let this = Rc::new(Self {
            window,
            toolbar_view,
            header_bar,
            search_entry,
            rail,
            nav_view,
            home_page,
            flow_box,
            section_title,
            chips_box,
            subs_strip_box,
            subs_avatars_row,
            history_header_box,
            toast_overlay,
            player_card,
            mpv_session: RefCell::new(None),
            mpv_pos: RefCell::new(0.0),
            mpv_duration: RefCell::new(0.0),
            history_store,
            settings_store,
            subscription_store,
            engine,
            cache,
            runtime,
        });

        // Wire Clear History Button
        let this_clear = this.clone();
        clear_hist_btn.connect_clicked(move |_| {
            this_clear.history_store.clear();
            this_clear
                .toast_overlay
                .add_toast(adw::Toast::new("Watch history cleared"));
            this_clear.load_history();
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
        // Search activation
        let this = self.clone();
        self.search_entry.connect_activate(move |entry| {
            let query = entry.text().trim().to_string();
            if !query.is_empty() {
                this.search(&query);
            }
        });

        // Rail Navigation
        let this = self.clone();
        self.rail.home_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::Home);
            this.load_trending();
        });

        let this = self.clone();
        self.rail.subs_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::Subscriptions);
            this.load_subscriptions();
        });

        let this = self.clone();
        self.rail.hist_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::History);
            this.load_history();
        });

        let this = self.clone();
        self.rail.settings_btn.connect_clicked(move |_| {
            this.rail.set_active(RailTab::Settings);
            this.open_settings();
        });

        // Chips category selection
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

                let this_play = this.clone();
                let sum_play = summary.clone();
                let on_play = move |_id: String, _title: String, quality: Quality| {
                    this_play.play_video(sum_play.clone(), quality);
                };

                let this_queue = this.clone();
                let on_add_queue = move |sum: VideoSummary| {
                    this_queue.queue_video(sum);
                };

                let this_chan = this.clone();
                let on_open_channel = move |cid: String, cname: String| {
                    this_chan.open_channel(&cid, &cname);
                };

                DetailsSheet::show(
                    &this.window,
                    &summary,
                    thumb_texture,
                    avatar_texture,
                    on_play,
                    on_add_queue,
                    on_toast,
                    on_open_channel,
                );
            }
        });

        // Player card controls wiring
        let this_ctrl = self.clone();
        self.player_card.connect_play_pause(move || {
            if let Some(session) = this_ctrl.mpv_session.borrow().as_ref() {
                session.toggle_pause();
            }
        });

        let this_next = self.clone();
        self.player_card.connect_next(move || {
            if let Some(session) = this_next.mpv_session.borrow().as_ref() {
                session.next();
            }
        });

        let this_prev = self.clone();
        self.player_card.connect_prev(move || {
            if let Some(session) = this_prev.mpv_session.borrow().as_ref() {
                session.prev();
            }
        });

        let this_clear = self.clone();
        self.player_card.connect_clear(move || {
            if let Some(session) = this_clear.mpv_session.borrow().as_ref() {
                session.clear();
            }
            this_clear.player_card.set_queue(vec![], 0);
        });

        let this_close = self.clone();
        self.player_card.connect_close(move || {
            if let Some(session) = this_close.mpv_session.borrow().as_ref() {
                session.stop();
            }
            this_close.player_card.set_queue(vec![], 0);
            this_close.player_card.root.set_visible(false);
        });
    }

    pub fn play_video(self: &Rc<Self>, summary: VideoSummary, quality: Quality) {
        self.ensure_mpv_session();

        let settings = self.settings_store.load();
        if settings.save_history {
            self.history_store.add(summary.clone());
        }

        let id = summary.id.clone();
        let title = summary.title.clone();
        if let Some(session) = self.mpv_session.borrow().as_ref() {
            session.play(&id, &title, quality);
        }

        self.player_card.set_queue(vec![summary.clone()], 0);
        self.player_card.set_playing(true);

        let q_name = match quality {
            Quality::P360 => "360p",
            Quality::P720 => "720p",
            Quality::P1080 => "1080p",
            Quality::AudioOnly => "Audio",
        };
        self.toast_overlay.add_toast(adw::Toast::new(&format!(
            "Playing \"{title}\" ({q_name}) in MPV..."
        )));
    }

    pub fn queue_video(self: &Rc<Self>, summary: VideoSummary) {
        self.ensure_mpv_session();

        let id = summary.id.clone();
        let title = summary.title.clone();
        if let Some(session) = self.mpv_session.borrow().as_ref() {
            session.append(&id, &title);
        }

        self.player_card.add_item(summary);
        self.toast_overlay.add_toast(adw::Toast::new("Added to queue"));
    }

    fn ensure_mpv_session(self: &Rc<Self>) {
        if self.mpv_session.borrow().is_some() {
            return;
        }

        let (sender, receiver) = async_channel::unbounded::<MpvEvent>();
        let (sess_tx, sess_rx) = async_channel::bounded::<Result<Arc<MpvSession>, String>>(1);

        self.runtime.spawn(async move {
            let res = MpvSession::start(sender).await;
            let _ = sess_tx.send(res).await;
        });

        let this_cl = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(Ok(session)) = sess_rx.recv().await {
                *this_cl.mpv_session.borrow_mut() = Some(session);
            }
        });

        let this_ev = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = receiver.recv().await {
                match event {
                    MpvEvent::TimePos(pos) => {
                        *this_ev.mpv_pos.borrow_mut() = pos;
                        let dur = *this_ev.mpv_duration.borrow();
                        this_ev.player_card.set_progress(pos, dur);
                    }
                    MpvEvent::Duration(dur) => {
                        *this_ev.mpv_duration.borrow_mut() = dur;
                        let pos = *this_ev.mpv_pos.borrow();
                        this_ev.player_card.set_progress(pos, dur);
                    }
                    MpvEvent::Pause(p) => {
                        this_ev.player_card.set_playing(!p);
                    }
                    MpvEvent::PlaylistPos(idx) => {
                        this_ev.player_card.set_current_index(idx);
                    }
                    MpvEvent::Idle(_idle) => {}
                    MpvEvent::ProcessExited => {
                        *this_ev.mpv_session.borrow_mut() = None;
                        this_ev.player_card.root.set_visible(false);
                    }
                }
            }
        });
    }

    pub fn open_channel(self: &Rc<Self>, channel_id: &str, channel_name: &str) -> Option<Rc<ChannelPage>> {
        if channel_id.is_empty() {
            return None;
        }

        let channel_page = ChannelPage::new(channel_id, channel_name);

        let this_all = self.clone();
        channel_page.connect_play_all(move |shorts| {
            if let Some(first) = shorts.first() {
                this_all.play_video(first.clone(), Quality::P360);
                for item in shorts.iter().skip(1) {
                    this_all.queue_video(item.clone());
                }
            }
        });

        self.nav_view.push(&channel_page.page);

        // Fetch channel videos and shorts
        let (ch_tx, ch_rx) = async_channel::bounded::<
            Result<(crate::backend::models::ChannelDetails, Vec<VideoSummary>), String>,
        >(1);
        let (sh_tx, sh_rx) =
            async_channel::bounded::<Result<Vec<VideoSummary>, String>>(1);

        let engine = self.engine.clone();
        let cid = channel_id.to_string();
        self.runtime.spawn(async move {
            let res = engine.channel_videos(&cid).await;
            let _ = ch_tx.send(res).await;
        });

        let engine = self.engine.clone();
        let cid2 = channel_id.to_string();
        self.runtime.spawn(async move {
            let res = engine.channel_shorts(&cid2).await;
            let _ = sh_tx.send(res).await;
        });

        let cp_cl = channel_page.clone();
        glib::spawn_future_local(async move {
            if let Ok(Ok((details, videos))) = ch_rx.recv().await {
                cp_cl.set_channel_details(details);
                cp_cl.set_videos(videos);
            }
        });

        let cp_sh = channel_page.clone();
        glib::spawn_future_local(async move {
            if let Ok(Ok(shorts)) = sh_rx.recv().await {
                cp_sh.set_shorts(shorts);
            }
        });

        Some(channel_page)
    }

    pub fn search(self: &Rc<Self>, query: &str) {
        self.nav_view.pop_to_page(&self.home_page);
        self.chips_box.set_visible(false);
        self.subs_strip_box.set_visible(false);
        self.history_header_box.set_visible(false);
        self.section_title.set_visible(true);
        self.section_title
            .set_text(&format!("Results for \"{query}\""));
        self.show_skeletons_search(6);

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
                            this.show_empty_state(
                                "No results found",
                                "Try different keywords or check spelling.",
                            );
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
        self.nav_view.pop_to_page(&self.home_page);
        self.chips_box.set_visible(true);
        self.subs_strip_box.set_visible(false);
        self.history_header_box.set_visible(false);
        self.section_title.set_visible(true);
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
        self.nav_view.pop_to_page(&self.home_page);
        self.chips_box.set_visible(true);
        self.subs_strip_box.set_visible(false);
        self.history_header_box.set_visible(false);
        self.section_title.set_visible(true);
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
                match res {
                    Ok(videos) => {
                        if videos.is_empty() {
                            this.show_empty_state("No trending videos", "Check back later.");
                        } else {
                            this.populate_videos(videos, ViewMode::Grid);
                        }
                    }
                    Err(e) => {
                        this.show_empty_state("Network Error", &e);
                    }
                }
            }
        });
    }

    pub fn load_subscriptions(self: &Rc<Self>) {
        self.nav_view.pop_to_page(&self.home_page);
        self.chips_box.set_visible(false);
        self.history_header_box.set_visible(false);
        self.subs_strip_box.set_visible(true);
        self.section_title.set_visible(true);
        self.section_title.set_text("Latest from Subscriptions");

        while let Some(child) = self.subs_avatars_row.first_child() {
            self.subs_avatars_row.remove(&child);
        }

        let subs = self.subscription_store.load_all();
        if subs.is_empty() {
            self.subs_strip_box.set_visible(false);
            self.section_title.set_text("Subscriptions");
            self.show_empty_state(
                "No Subscriptions Yet",
                "Channels you subscribe to will appear here.",
            );
            return;
        }

        for sub in subs.iter() {
            let item_box = GtkBox::new(Orientation::Vertical, 6);
            item_box.add_css_class("yr-avatar-btn");
            item_box.set_valign(Align::Center);

            let av = adw::Avatar::new(56, Some(&sub.name), true);
            av.set_valign(Align::Center);

            let lbl = Label::new(Some(&sub.name));
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            lbl.set_max_width_chars(11);
            lbl.add_css_class("yr-meta");

            item_box.append(&av);
            item_box.append(&lbl);

            let click = gtk4::GestureClick::new();
            let cid = sub.id.clone();
            let cname = sub.name.clone();
            let this_chan = self.clone();
            click.connect_released(move |_, _, _, _| {
                this_chan.open_channel(&cid, &cname);
            });
            item_box.add_controller(click);

            self.subs_avatars_row.append(&item_box);
        }

        if let Some(first_sub) = subs.first() {
            self.show_skeletons(8);
            let (sender, receiver) =
                async_channel::unbounded::<Result<Vec<VideoSummary>, String>>();
            let engine = self.engine.clone();
            let cid = first_sub.id.clone();
            self.runtime.spawn(async move {
                let res = engine.channel_videos(&cid).await;
                let _ = sender.send(res.map(|(_, vids)| vids)).await;
            });

            let this = self.clone();
            glib::spawn_future_local(async move {
                if let Ok(res) = receiver.recv().await {
                    match res {
                        Ok(videos) => this.populate_videos(videos, ViewMode::Grid),
                        Err(_) => this.show_empty_state(
                            "No Videos Found",
                            "No recent videos from your subscriptions.",
                        ),
                    }
                }
            });
        }
    }

    pub fn load_history(self: &Rc<Self>) {
        self.nav_view.pop_to_page(&self.home_page);
        self.chips_box.set_visible(false);
        self.subs_strip_box.set_visible(false);
        self.section_title.set_visible(false);
        self.history_header_box.set_visible(true);

        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        let entries = self.history_store.load_all();
        if entries.is_empty() {
            self.show_empty_state(
                "No Watch History",
                "Videos you watch will appear here.",
            );
            return;
        }

        self.flow_box.set_max_children_per_line(1);
        self.flow_box.set_min_children_per_line(1);
        self.flow_box.set_column_spacing(0);
        self.flow_box.set_row_spacing(12);

        for entry in entries {
            let this_rm = self.clone();
            let vid = entry.video.id.clone();
            let (child, picture) = VideoCard::new_history(&entry, move || {
                this_rm.history_store.remove(&vid);
                this_rm.load_history();
            });

            unsafe {
                child.set_data("video_summary", entry.video.clone());
            }

            self.load_thumbnail_async_pic(
                &picture,
                Some(&child),
                &entry.video.thumbnail_url,
                &entry.video.id,
            );

            let toast_overlay = self.toast_overlay.clone();
            let on_toast = move |msg: String| {
                toast_overlay.add_toast(adw::Toast::new(&msg));
            };

            let this_play = self.clone();
            let sum_play = entry.video.clone();
            let on_play = move |_id: String, _title: String, quality: Quality| {
                this_play.play_video(sum_play.clone(), quality);
            };

            let this_queue = self.clone();
            let on_add_queue = move |sum: VideoSummary| {
                this_queue.queue_video(sum);
            };

            let this_chan = self.clone();
            let on_open_channel = move |cid: String, cname: String| {
                this_chan.open_channel(&cid, &cname);
            };

            ContextMenu::attach(
                &child,
                &entry.video,
                on_play,
                on_add_queue,
                on_toast,
                on_open_channel,
            );

            self.flow_box.append(&child);
        }
    }

    pub fn open_settings(self: &Rc<Self>) {
        let prefs = adw::PreferencesDialog::builder()
            .title("Settings")
            .build();

        let page = adw::PreferencesPage::builder()
            .title("General")
            .icon_name("emblem-system-symbolic")
            .build();

        let group_playback = adw::PreferencesGroup::builder()
            .title("Playback")
            .description("Configure video playback and external player")
            .build();

        let settings = self.settings_store.load();

        let quality_model = gtk4::StringList::new(&["360p", "720p", "1080p"]);
        let selected_idx = match settings.default_quality.as_str() {
            "720p" => 1,
            "1080p" => 2,
            _ => 0,
        };
        let quality_row = adw::ComboRow::builder()
            .title("Default Resolution")
            .subtitle("Preferred starting stream resolution")
            .model(&quality_model)
            .selected(selected_idx)
            .build();

        let store_q = self.settings_store.clone();
        quality_row.connect_selected_notify(move |row| {
            let mut s = store_q.load();
            s.default_quality = match row.selected() {
                1 => "720p".to_string(),
                2 => "1080p".to_string(),
                _ => "360p".to_string(),
            };
            store_q.save(&s);
        });

        let hwdec_row = adw::SwitchRow::builder()
            .title("Hardware Video Decoding")
            .subtitle("Enable VA-API hardware acceleration in MPV")
            .active(settings.hardware_decoding)
            .build();

        let store_hw = self.settings_store.clone();
        hwdec_row.connect_active_notify(move |row| {
            let mut s = store_hw.load();
            s.hardware_decoding = row.is_active();
            store_hw.save(&s);
        });

        let player_row = adw::EntryRow::builder()
            .title("External Player Executable")
            .text(&settings.external_player)
            .build();

        let store_pl = self.settings_store.clone();
        player_row.connect_changed(move |row| {
            let mut s = store_pl.load();
            s.external_player = row.text().to_string();
            store_pl.save(&s);
        });

        group_playback.add(&quality_row);
        group_playback.add(&hwdec_row);
        group_playback.add(&player_row);

        let group_data = adw::PreferencesGroup::builder()
            .title("Data &amp; Storage")
            .description("Manage local cache and watching history")
            .build();

        let hist_row = adw::SwitchRow::builder()
            .title("Save Watch History")
            .subtitle("Locally record played videos in history")
            .active(settings.save_history)
            .build();

        let store_h = self.settings_store.clone();
        hist_row.connect_active_notify(move |row| {
            let mut s = store_h.load();
            s.save_history = row.is_active();
            store_h.save(&s);
        });

        let clear_cache_row = adw::ActionRow::builder()
            .title("Clear Thumbnail Cache")
            .subtitle("Removes cached video thumbnails from disk")
            .build();
        let clear_cache_btn = Button::with_label("Clear Cache");
        clear_cache_btn.add_css_class("yr-btn");
        clear_cache_btn.set_valign(Align::Center);
        let toast_overlay = self.toast_overlay.clone();
        let cache_ref = self.cache.clone();
        clear_cache_btn.connect_clicked(move |_| {
            cache_ref.clear_all();
            toast_overlay.add_toast(adw::Toast::new("Thumbnail cache cleared"));
        });
        clear_cache_row.add_suffix(&clear_cache_btn);

        let clear_hist_row = adw::ActionRow::builder()
            .title("Clear Watch History")
            .subtitle("Deletes all recorded watch history")
            .build();
        let clear_hist_btn = Button::with_label("Clear History");
        clear_hist_btn.add_css_class("yr-btn");
        clear_hist_btn.set_valign(Align::Center);
        let toast_overlay2 = self.toast_overlay.clone();
        let hist_store_ref = self.history_store.clone();
        let this_hist = self.clone();
        clear_hist_btn.connect_clicked(move |_| {
            hist_store_ref.clear();
            toast_overlay2.add_toast(adw::Toast::new("Watch history cleared"));
            this_hist.load_history();
        });
        clear_hist_row.add_suffix(&clear_hist_btn);

        group_data.add(&hist_row);
        group_data.add(&clear_cache_row);
        group_data.add(&clear_hist_row);

        page.add(&group_playback);
        page.add(&group_data);
        prefs.add(&page);
        prefs.present(Some(&self.window));
    }

    fn show_skeletons(self: &Rc<Self>, count: usize) {
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.flow_box.set_max_children_per_line(4);
        self.flow_box.set_min_children_per_line(1);
        self.flow_box.set_column_spacing(20);
        self.flow_box.set_row_spacing(24);
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_skeleton());
        }
    }

    fn show_skeletons_search(self: &Rc<Self>, count: usize) {
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.flow_box.set_max_children_per_line(1);
        self.flow_box.set_min_children_per_line(1);
        self.flow_box.set_column_spacing(0);
        self.flow_box.set_row_spacing(14);
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_search_skeleton());
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

    pub fn populate_videos(self: &Rc<Self>, videos: Vec<VideoSummary>, mode: ViewMode) {
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

            let this_play = self.clone();
            let sum_play = summary.clone();
            let on_play = move |_id: String, _title: String, quality: Quality| {
                this_play.play_video(sum_play.clone(), quality);
            };

            let this_queue = self.clone();
            let on_add_queue = move |sum: VideoSummary| {
                this_queue.queue_video(sum);
            };

            let this_chan = self.clone();
            let on_open_channel = move |cid: String, cname: String| {
                this_chan.open_channel(&cid, &cname);
            };

            ContextMenu::attach(
                &card.container,
                &summary,
                on_play,
                on_add_queue,
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

    fn load_thumbnail_async_pic(
        self: &Rc<Self>,
        picture: &Picture,
        container: Option<&gtk4::FlowBoxChild>,
        url: &str,
        id: &str,
    ) {
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

        let pic_clone = picture.clone();
        let cont_clone = container.cloned();
        glib::spawn_future_local(async move {
            if let Ok(Some(bytes)) = receiver.recv().await {
                let glib_bytes = glib::Bytes::from(&bytes);
                if let Ok(texture) = Texture::from_bytes(&glib_bytes) {
                    pic_clone.set_paintable(Some(&texture));
                    if let Some(c) = cont_clone {
                        unsafe {
                            c.set_data("thumb_texture", texture);
                        }
                    }
                }
            }
        });
    }

    fn load_thumbnail_async(self: &Rc<Self>, card: &VideoCard, url: &str, id: &str) {
        self.load_thumbnail_async_pic(&card.picture, Some(&card.container), url, id);
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
    provider.load_from_string(css);

    if let Some(display) = Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
