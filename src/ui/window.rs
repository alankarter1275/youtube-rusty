use crate::backend::cache::Cache;
use crate::backend::engine::{download_with_ytdlp, Engine};
use crate::backend::history::HistoryStore;
use crate::backend::models::{
    DurationFilter, SearchSortOrder, UploadDateFilter, VideoSummary,
};
use crate::backend::settings::{AppSettings, SettingsStore};
use crate::backend::subscriptions::SubscriptionStore;
use crate::backend::watch_later::WatchLaterStore;
use crate::player::mpv::Quality;
use crate::player::mpv_ipc::{MpvEvent, MpvSession};
use crate::ui::channel_page::ChannelPage;
use crate::ui::details_sheet::{CardActions, ContextMenu, DetailsSheet};
use crate::ui::image_loader::ImageLoader;
use crate::ui::player_card::PlayerCard;
use crate::ui::rail::{Rail, RailTab};
use crate::ui::video_card::VideoCard;
use adw::prelude::*;
use gtk4::gdk::{self, Display};
use gtk4::{
    gio, glib, Align, Box as GtkBox, Button, CssProvider, Editable, EventControllerKey, FlowBox,
    GestureClick, Image, Label, MenuButton, Orientation, Overlay, Popover, ScrolledWindow,
    SearchEntry, SelectionMode, Separator,
};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Runtime;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    SearchList,
}

#[derive(Clone, PartialEq, Eq)]
enum ActiveView {
    HomeTrending,
    HomeCategory(String),
    WatchLater,
    Search(String),
    Subscriptions,
    History,
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    back_btn: Button,
    pub search_entry: SearchEntry,
    pub rail: Rail,
    bottom_home: Button,
    bottom_subs: Button,
    bottom_hist: Button,
    bottom_settings: Button,
    pub nav_view: adw::NavigationView,
    pub home_page: adw::NavigationPage,
    pub flow_box: FlowBox,
    status_box: GtkBox,
    status_icon: Image,
    status_title: Label,
    status_desc: Label,
    status_action_btn: Button,
    status_action_cb: RefCell<Option<Rc<dyn Fn()>>>,
    section_header_row: GtkBox,
    pub section_title: Label,
    search_filters_btn: Button,
    clear_watch_later_btn: Button,
    chips_scroll: ScrolledWindow,
    chip_trending: Button,
    chip_gaming: Button,
    chip_music: Button,
    chip_news: Button,
    chip_watch_later: Button,
    pub subs_strip_box: GtkBox,
    pub subs_avatars_row: GtkBox,
    pub history_header_box: GtkBox,
    pub toast_overlay: adw::ToastOverlay,
    pub player_card: Rc<PlayerCard>,
    pub mpv_session: RefCell<Option<Arc<MpvSession>>>,
    mpv_player_cmd: RefCell<String>,
    mpv_became_active: Cell<bool>,
    pub mpv_pos: Cell<f64>,
    pub mpv_duration: Cell<f64>,
    is_narrow: Cell<bool>,
    view_mode: Cell<ViewMode>,
    active_view: RefCell<ActiveView>,
    request_seq: Cell<u64>,
    search_date_filter: Cell<UploadDateFilter>,
    search_dur_filter: Cell<DurationFilter>,
    search_sort_order: Cell<SearchSortOrder>,
    open_channels: RefCell<Vec<Rc<ChannelPage>>>,
    theme_provider: CssProvider,
    pub image_loader: Rc<ImageLoader>,
    pub history_store: Arc<HistoryStore>,
    pub settings_store: Arc<SettingsStore>,
    pub subscription_store: Arc<SubscriptionStore>,
    pub watch_later_store: Arc<WatchLaterStore>,
    engine: Arc<Engine>,
    runtime: Arc<Runtime>,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let theme_provider = load_custom_css();

        // HeaderBar: borderless, blends into background
        let header_bar = adw::HeaderBar::new();
        header_bar.set_show_title(false);

        let back_btn = Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Back (Alt+Left)")
            .visible(false)
            .build();
        back_btn.add_css_class("flat");
        header_bar.pack_start(&back_btn);

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
        menu_btn.set_tooltip_text(Some("Main menu"));
        menu_btn.add_css_class("flat");

        let menu = gio::Menu::new();
        menu.append(Some("Preferences"), Some("app.settings"));
        menu.append(Some("Watch Later"), Some("app.watch_later"));
        menu.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        menu.append(Some("About youtube-rusty"), Some("app.about"));
        menu_btn.set_menu_model(Some(&menu));
        header_bar.pack_end(&menu_btn);

        // Left Navigation Rail (68px)
        let rail = Rail::new();

        // Scrollable Chips Row
        let chips_box = GtkBox::new(Orientation::Horizontal, 8);
        chips_box.set_margin_start(28);
        chips_box.set_margin_end(28);
        chips_box.set_margin_top(16);
        chips_box.set_margin_bottom(12);

        let chip_trending = create_chip("Trending", true);
        let chip_gaming = create_chip("Gaming", false);
        let chip_music = create_chip("Music", false);
        let chip_news = create_chip("News", false);
        let chip_watch_later = create_chip("Watch later", false);

        chips_box.append(&chip_trending);
        chips_box.append(&chip_gaming);
        chips_box.append(&chip_music);
        chips_box.append(&chip_news);
        chips_box.append(&chip_watch_later);

        let chips_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .child(&chips_box)
            .build();

        // Subscriptions Avatar Strip
        let subs_strip_box = GtkBox::new(Orientation::Vertical, 8);
        subs_strip_box.set_margin_start(28);
        subs_strip_box.set_margin_end(28);
        subs_strip_box.set_margin_top(16);
        subs_strip_box.set_margin_bottom(12);
        subs_strip_box.set_visible(false);

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

        let hist_title = Label::new(Some("History"));
        hist_title.set_halign(Align::Start);
        hist_title.set_xalign(0.0);
        hist_title.set_hexpand(true);
        hist_title.add_css_class("yr-h1");

        let clear_hist_btn = Button::with_label("Clear history");
        clear_hist_btn.add_css_class("yr-chip");
        clear_hist_btn.set_valign(Align::Center);

        history_header_box.append(&hist_title);
        history_header_box.append(&clear_hist_btn);

        // Section Header Row (Title + Search Filters / Clear Watch Later)
        let section_header_row = GtkBox::new(Orientation::Horizontal, 12);
        section_header_row.set_margin_start(28);
        section_header_row.set_margin_end(28);
        section_header_row.set_margin_bottom(16);
        section_header_row.set_valign(Align::Center);

        let section_title = Label::new(Some("Trending"));
        section_title.set_halign(Align::Start);
        section_title.set_xalign(0.0);
        section_title.set_hexpand(true);
        section_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        section_title.add_css_class("yr-h2");

        let search_filters_btn = Button::with_label("Filters");
        search_filters_btn.add_css_class("yr-chip");
        search_filters_btn.set_tooltip_text(Some("Filter search results"));
        search_filters_btn.set_visible(false);

        let clear_watch_later_btn = Button::with_label("Clear saved");
        clear_watch_later_btn.add_css_class("yr-chip");
        clear_watch_later_btn.set_visible(false);

        section_header_row.append(&section_title);
        section_header_row.append(&search_filters_btn);
        section_header_row.append(&clear_watch_later_btn);

        // Full-width Empty / Error Status View (matches Empty no results.png)
        let status_box = GtkBox::new(Orientation::Vertical, 12);
        status_box.set_hexpand(true);
        status_box.set_vexpand(true);
        status_box.set_halign(Align::Center);
        status_box.set_valign(Align::Center);
        status_box.set_margin_top(64);
        status_box.set_margin_bottom(64);
        status_box.set_margin_start(24);
        status_box.set_margin_end(24);
        status_box.set_visible(false);

        let icon_circle = GtkBox::new(Orientation::Vertical, 0);
        icon_circle.add_css_class("yr-status-icon");
        icon_circle.set_halign(Align::Center);
        icon_circle.set_valign(Align::Center);
        let status_icon = Image::from_icon_name("system-search-symbolic");
        status_icon.set_pixel_size(36);
        status_icon.set_halign(Align::Center);
        status_icon.set_valign(Align::Center);
        status_icon.set_vexpand(true);
        icon_circle.append(&status_icon);

        let status_title = Label::new(Some("No results"));
        status_title.add_css_class("yr-h1");
        status_title.set_halign(Align::Center);

        let status_desc = Label::new(Some(""));
        status_desc.add_css_class("yr-meta");
        status_desc.set_halign(Align::Center);
        status_desc.set_justify(gtk4::Justification::Center);
        status_desc.set_wrap(true);
        status_desc.set_max_width_chars(48);

        let status_action_btn = Button::with_label("Retry");
        status_action_btn.add_css_class("yr-btn");
        status_action_btn.add_css_class("primary");
        status_action_btn.set_halign(Align::Center);
        status_action_btn.set_margin_top(6);

        status_box.append(&icon_circle);
        status_box.append(&status_title);
        status_box.append(&status_desc);
        status_box.append(&status_action_btn);

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
        content_v_box.set_vexpand(true);
        content_v_box.append(&chips_scroll);
        content_v_box.append(&subs_strip_box);
        content_v_box.append(&history_header_box);
        content_v_box.append(&section_header_row);
        content_v_box.append(&status_box);
        content_v_box.append(&flow_box);

        let content_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&content_v_box)
            .vexpand(true)
            .hexpand(true)
            .build();

        let nav_view = adw::NavigationView::new();
        let home_page = adw::NavigationPage::builder()
            .title("Home")
            .tag("home")
            .child(&content_scrolled)
            .build();
        nav_view.push(&home_page);

        let main_h_box = GtkBox::new(Orientation::Horizontal, 0);
        main_h_box.append(&rail.container);
        main_h_box.append(&nav_view);

        let cache = Arc::new(Cache::new());
        let runtime = Arc::new(Runtime::new().expect("Failed to initialize Tokio runtime"));
        let engine = Arc::new(Engine::new(cache.root_dir()));
        let image_loader = ImageLoader::new(cache, runtime.clone());

        let content_overlay = Overlay::new();
        content_overlay.set_child(Some(&main_h_box));

        let player_card = PlayerCard::new(image_loader.clone());
        content_overlay.add_overlay(&player_card.root);

        // Bottom Navigation Bar for Narrow Breakpoint (< 600px)
        let bottom_bar = GtkBox::new(Orientation::Horizontal, 0);
        bottom_bar.add_css_class("yr-bottom-bar");
        bottom_bar.set_visible(false);

        let bottom_home = create_bottom_nav_btn("user-home-symbolic", "Home", true);
        let bottom_subs = create_bottom_nav_btn("emblem-favorite-symbolic", "Subscriptions", false);
        let bottom_hist = create_bottom_nav_btn("document-open-recent-symbolic", "History", false);
        let bottom_settings = create_bottom_nav_btn("emblem-system-symbolic", "Settings", false);

        bottom_bar.append(&bottom_home);
        bottom_bar.append(&bottom_subs);
        bottom_bar.append(&bottom_hist);
        bottom_bar.append(&bottom_settings);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header_bar);
        toolbar_view.add_bottom_bar(&bottom_bar);
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

        let history_store = Arc::new(HistoryStore::new());
        let settings_store = Arc::new(SettingsStore::new());
        let subscription_store = Arc::new(SubscriptionStore::new());
        let watch_later_store = Arc::new(WatchLaterStore::new());

        let initial_settings = settings_store.load();

        let this = Rc::new(Self {
            window,
            back_btn,
            search_entry,
            rail,
            bottom_home,
            bottom_subs,
            bottom_hist,
            bottom_settings,
            nav_view,
            home_page,
            flow_box,
            status_box,
            status_icon,
            status_title,
            status_desc,
            status_action_btn,
            status_action_cb: RefCell::new(None),
            section_header_row,
            section_title,
            search_filters_btn,
            clear_watch_later_btn,
            chips_scroll,
            chip_trending,
            chip_gaming,
            chip_music,
            chip_news,
            chip_watch_later,
            subs_strip_box,
            subs_avatars_row,
            history_header_box,
            toast_overlay,
            player_card,
            mpv_session: RefCell::new(None),
            mpv_player_cmd: RefCell::new(initial_settings.external_player.clone()),
            mpv_became_active: Cell::new(false),
            mpv_pos: Cell::new(0.0),
            mpv_duration: Cell::new(0.0),
            is_narrow: Cell::new(false),
            view_mode: Cell::new(ViewMode::Grid),
            active_view: RefCell::new(ActiveView::HomeTrending),
            request_seq: Cell::new(0),
            search_date_filter: Cell::new(UploadDateFilter::AnyTime),
            search_dur_filter: Cell::new(DurationFilter::AnyLength),
            search_sort_order: Cell::new(SearchSortOrder::Relevance),
            open_channels: RefCell::new(Vec::new()),
            theme_provider,
            image_loader,
            history_store,
            settings_store,
            subscription_store,
            watch_later_store,
            engine,
            runtime,
        });

        this.apply_appearance(&initial_settings);
        this.register_app_actions(app);
        this.setup_breakpoint(&bottom_bar);
        this.setup_search_filters_popover();
        this.setup_events(&clear_hist_btn);
        this.load_trending();

        this
    }

    fn apply_appearance(&self, settings: &AppSettings) {
        let style_manager = adw::StyleManager::default();
        let is_light = match settings.color_scheme.as_str() {
            "light" => {
                style_manager.set_color_scheme(adw::ColorScheme::ForceLight);
                true
            }
            "system" => {
                style_manager.set_color_scheme(adw::ColorScheme::Default);
                !style_manager.is_dark()
            }
            _ => {
                style_manager.set_color_scheme(adw::ColorScheme::PreferDark);
                false
            }
        };

        let (accent, accent_hi) = match settings.accent_color.as_str() {
            "forest" => ("#3D8B5E", "#68B688"),
            "plum" => ("#8E4A7E", "#BA78AA"),
            "teal" => ("#2D8A8A", "#5DB4B4"),
            _ => ("#D4552A", "#E8845F"),
        };

        let css = if is_light {
            format!(
                "@define-color yr_bg #FAF6F2;
                 @define-color yr_surface #EFE7DF;
                 @define-color yr_text #1C1512;
                 @define-color yr_text2 #5C4F47;
                 @define-color yr_accent {accent};
                 @define-color yr_accent_hi {accent};
                 @define-color yr_on_accent #FAF6F2;
                 .yr-thumb {{ background-color: #E3D7CC; }}
                 .yr-card:hover .yr-thumb {{ background-color: #D8C9BC; }}"
            )
        } else {
            format!(
                "@define-color yr_bg #100C0A;
                 @define-color yr_surface #1A1512;
                 @define-color yr_text #F4EDE7;
                 @define-color yr_text2 #C9BEB6;
                 @define-color yr_accent {accent};
                 @define-color yr_accent_hi {accent_hi};
                 @define-color yr_on_accent #100C0A;"
            )
        };

        self.theme_provider.load_from_string(&css);
    }

    fn register_app_actions(self: &Rc<Self>, app: &adw::Application) {
        let weak = Rc::downgrade(self);
        let act_settings = gio::SimpleAction::new("settings", None);
        act_settings.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.open_settings();
            }
        });
        app.add_action(&act_settings);

        let weak = Rc::downgrade(self);
        let act_wl = gio::SimpleAction::new("watch_later", None);
        act_wl.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::Home);
                w.highlight_chip(Some(&w.chip_watch_later));
                w.load_watch_later();
            }
        });
        app.add_action(&act_wl);

        let weak = Rc::downgrade(self);
        let act_shortcuts = gio::SimpleAction::new("shortcuts", None);
        act_shortcuts.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.show_shortcuts_dialog();
            }
        });
        app.add_action(&act_shortcuts);

        let weak = Rc::downgrade(self);
        let act_about = gio::SimpleAction::new("about", None);
        act_about.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.show_about_dialog();
            }
        });
        app.add_action(&act_about);
    }

    fn setup_breakpoint(self: &Rc<Self>, bottom_bar: &GtkBox) {
        if let Ok(cond) = adw::BreakpointCondition::parse("max-width: 600px") {
            let breakpoint = adw::Breakpoint::new(cond);

            let weak_narrow = Rc::downgrade(self);
            let b_bar_narrow = bottom_bar.clone();
            breakpoint.connect_apply(move |_| {
                if let Some(w) = weak_narrow.upgrade() {
                    w.is_narrow.set(true);
                    w.rail.container.set_visible(false);
                    b_bar_narrow.set_visible(true);
                    w.player_card.set_narrow_mode(true);
                    w.update_flow_columns();
                    w.notify_channels_narrow(true);
                }
            });

            let weak_wide = Rc::downgrade(self);
            let b_bar_wide = bottom_bar.clone();
            breakpoint.connect_unapply(move |_| {
                if let Some(w) = weak_wide.upgrade() {
                    w.is_narrow.set(false);
                    w.rail.container.set_visible(true);
                    b_bar_wide.set_visible(false);
                    w.player_card.set_narrow_mode(false);
                    w.update_flow_columns();
                    w.notify_channels_narrow(false);
                }
            });

            self.window.add_breakpoint(breakpoint);
        }
    }

    fn notify_channels_narrow(&self, narrow: bool) {
        for cp in self.open_channels.borrow().iter() {
            cp.set_narrow_mode(narrow);
        }
    }

    fn set_active_nav(&self, tab: RailTab) {
        self.rail.set_active(tab);
        self.bottom_home.remove_css_class("active");
        self.bottom_subs.remove_css_class("active");
        self.bottom_hist.remove_css_class("active");
        self.bottom_settings.remove_css_class("active");

        match tab {
            RailTab::Home => self.bottom_home.add_css_class("active"),
            RailTab::Subscriptions => self.bottom_subs.add_css_class("active"),
            RailTab::History => self.bottom_hist.add_css_class("active"),
            RailTab::Settings => self.bottom_settings.add_css_class("active"),
        }
    }

    fn highlight_chip(&self, target: Option<&Button>) {
        for c in [
            &self.chip_trending,
            &self.chip_gaming,
            &self.chip_music,
            &self.chip_news,
            &self.chip_watch_later,
        ] {
            c.remove_css_class("active");
        }
        if let Some(btn) = target {
            btn.add_css_class("active");
        }
    }

    fn update_flow_columns(&self) {
        let narrow = self.is_narrow.get();
        self.flow_box.set_margin_start(if narrow { 16 } else { 28 });
        self.flow_box.set_margin_end(if narrow { 16 } else { 28 });

        match self.view_mode.get() {
            ViewMode::Grid => {
                self.flow_box
                    .set_max_children_per_line(if narrow { 1 } else { 4 });
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
    }

    fn setup_events(self: &Rc<Self>, clear_hist_btn: &Button) {
        // Ensure MPV is cleanly shut down when window closes
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(w) = weak.upgrade() {
                if let Some(sess) = w.mpv_session.borrow_mut().take() {
                    sess.shutdown();
                }
            }
            glib::Propagation::Proceed
        });

        // Window keyboard shortcuts: '/', 'Ctrl+F', 'Alt+Left'
        let key_ctrl = EventControllerKey::new();
        let weak = Rc::downgrade(self);
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };

            if mods.contains(gdk::ModifierType::CONTROL_MASK)
                && (key == gdk::Key::f || key == gdk::Key::F)
            {
                w.search_entry.grab_focus();
                return glib::Propagation::Stop;
            }

            if mods.contains(gdk::ModifierType::ALT_MASK) && key == gdk::Key::Left {
                if w.back_btn.is_visible() {
                    w.nav_view.pop();
                    return glib::Propagation::Stop;
                }
            }

            if key == gdk::Key::slash
                && !mods.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                )
            {
                let in_editable = w
                    .window
                    .focus()
                    .map(|f| f.is::<Editable>())
                    .unwrap_or(false);
                if !in_editable {
                    w.search_entry.grab_focus();
                    return glib::Propagation::Stop;
                }
            }

            if key == gdk::Key::Escape {
                let search_focused = w
                    .window
                    .focus()
                    .map(|f| {
                        f.is_ancestor(&w.search_entry)
                            || &f == w.search_entry.upcast_ref::<gtk4::Widget>()
                    })
                    .unwrap_or(false);
                if search_focused {
                    w.window.set_focus(gtk4::Widget::NONE);
                    return glib::Propagation::Stop;
                }
            }

            glib::Propagation::Proceed
        });
        self.window.add_controller(key_ctrl);

        // Header Back Button + NavigationView page tracking
        let nav_for_back = self.nav_view.clone();
        self.back_btn.connect_clicked(move |_| {
            nav_for_back.pop();
        });

        let weak = Rc::downgrade(self);
        self.nav_view.connect_visible_page_notify(move |nv| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let is_home = nv
                .visible_page()
                .and_then(|p| p.tag())
                .map(|t| t.as_str() == "home")
                .unwrap_or(true);
            w.back_btn.set_visible(!is_home);

            if is_home {
                w.open_channels.borrow_mut().clear();
                if *w.active_view.borrow() == ActiveView::Subscriptions {
                    w.load_subscriptions();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.nav_view.connect_popped(move |_, popped_page| {
            if let Some(w) = weak.upgrade() {
                w.open_channels
                    .borrow_mut()
                    .retain(|cp| &cp.page != popped_page);
            }
        });

        // Status page action button ("Retry" / "Clear search")
        let weak = Rc::downgrade(self);
        self.status_action_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                if let Some(cb) = w.status_action_cb.borrow().as_ref().cloned() {
                    cb();
                }
            }
        });

        // Search activation
        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |entry| {
            let query = entry.text().trim().to_string();
            if !query.is_empty() {
                if let Some(w) = weak.upgrade() {
                    w.search(&query);
                }
            }
        });

        // Clear History button
        let weak = Rc::downgrade(self);
        clear_hist_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.history_store.clear();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Watch history cleared"));
                w.load_history();
            }
        });

        // Clear Watch Later button
        let weak = Rc::downgrade(self);
        self.clear_watch_later_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.watch_later_store.clear();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Cleared saved Watch Later videos"));
                w.load_watch_later();
            }
        });

        // Rail & Logo Navigation
        let weak = Rc::downgrade(self);
        self.rail.logo_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::Home);
                w.highlight_chip(Some(&w.chip_trending));
                w.load_trending();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.home_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::Home);
                w.highlight_chip(Some(&w.chip_trending));
                w.load_trending();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.subs_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::Subscriptions);
                w.load_subscriptions();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.hist_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::History);
                w.load_history();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.settings_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.open_settings();
            }
        });

        // Narrow Bottom Bar Navigation
        let weak = Rc::downgrade(self);
        self.bottom_home.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::Home);
                w.highlight_chip(Some(&w.chip_trending));
                w.load_trending();
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_subs.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::Subscriptions);
                w.load_subscriptions();
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_hist.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set_active_nav(RailTab::History);
                w.load_history();
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_settings.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.open_settings();
            }
        });

        // Chips Category Selection
        let weak = Rc::downgrade(self);
        self.chip_trending.connect_clicked(move |btn| {
            if let Some(w) = weak.upgrade() {
                w.highlight_chip(Some(btn));
                w.load_trending();
            }
        });

        let weak = Rc::downgrade(self);
        self.chip_gaming.connect_clicked(move |btn| {
            if let Some(w) = weak.upgrade() {
                w.highlight_chip(Some(btn));
                w.search_category("Gaming");
            }
        });

        let weak = Rc::downgrade(self);
        self.chip_music.connect_clicked(move |btn| {
            if let Some(w) = weak.upgrade() {
                w.highlight_chip(Some(btn));
                w.search_category("Music");
            }
        });

        let weak = Rc::downgrade(self);
        self.chip_news.connect_clicked(move |btn| {
            if let Some(w) = weak.upgrade() {
                w.highlight_chip(Some(btn));
                w.search_category("News");
            }
        });

        let weak = Rc::downgrade(self);
        self.chip_watch_later.connect_clicked(move |btn| {
            if let Some(w) = weak.upgrade() {
                w.highlight_chip(Some(btn));
                w.load_watch_later();
            }
        });

        // Player Card Controls Wiring
        let weak = Rc::downgrade(self);
        self.player_card.connect_play_pause(move || {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.toggle_pause();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_next(move || {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.next();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_prev(move || {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.prev();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_jump_to(move |idx| {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.jump_to(idx);
                    if let Some(cur) = w.player_card.current_video() {
                        session.set_media_title(&cur.title);
                    }
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_remove_at(move |idx| {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.remove_at(idx);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_move_item(move |from_idx, to_idx| {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.move_item(from_idx, to_idx);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_empty_stop(move || {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.stop();
                    session.clear();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_open_channel(move |cid, cname| {
            if let Some(w) = weak.upgrade() {
                w.open_channel(&cid, &cname);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_clear(move || {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.stop();
                    session.clear();
                }
                w.player_card.set_queue(vec![], 0, false);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_close(move || {
            if let Some(w) = weak.upgrade() {
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.stop();
                    session.clear();
                }
                w.player_card.set_queue(vec![], 0, false);
                w.player_card.root.set_visible(false);
            }
        });
    }

    fn setup_search_filters_popover(self: &Rc<Self>) {
        let popover = Popover::new();
        popover.set_parent(&self.search_filters_btn);
        popover.set_has_arrow(false);

        let pop_cleanup = popover.clone();
        self.search_filters_btn.connect_destroy(move |_| {
            pop_cleanup.unparent();
        });

        let content = GtkBox::new(Orientation::Vertical, 2);
        content.set_width_request(210);

        let h_date = Label::new(Some("UPLOAD DATE"));
        h_date.add_css_class("yr-popover-head");
        h_date.set_halign(Align::Start);
        content.append(&h_date);

        let date_specs = [
            (UploadDateFilter::AnyTime, "Any time"),
            (UploadDateFilter::Today, "Today"),
            (UploadDateFilter::ThisWeek, "This week"),
            (UploadDateFilter::ThisMonth, "This month"),
            (UploadDateFilter::ThisYear, "This year"),
        ];
        let pending_date = Rc::new(Cell::new(UploadDateFilter::AnyTime));
        let mut date_btns = Vec::new();
        for (v, lbl) in date_specs {
            let sel = v == UploadDateFilter::AnyTime;
            let btn = make_filter_item(&if sel { format!("{lbl} ✓") } else { lbl.to_string() }, sel);
            content.append(&btn);
            date_btns.push((v, lbl, btn));
        }
        let date_btns = Rc::new(date_btns);
        for (v, _, btn) in date_btns.iter() {
            let val = *v;
            let pd = pending_date.clone();
            let all = date_btns.clone();
            btn.connect_clicked(move |_| {
                pd.set(val);
                for (iv, il, b) in all.iter() {
                    let s = *iv == val;
                    update_filter_item(b, &if s { format!("{il} ✓") } else { il.to_string() }, s);
                }
            });
        }

        let sep1 = Separator::new(Orientation::Horizontal);
        sep1.set_margin_top(4);
        sep1.set_margin_bottom(4);
        content.append(&sep1);

        let h_dur = Label::new(Some("DURATION"));
        h_dur.add_css_class("yr-popover-head");
        h_dur.set_halign(Align::Start);
        content.append(&h_dur);

        let dur_specs = [
            (DurationFilter::AnyLength, "Any length"),
            (DurationFilter::Under4Min, "Under 4 min"),
            (DurationFilter::FourTo20Min, "4 to 20 min"),
            (DurationFilter::Over20Min, "Over 20 min"),
        ];
        let pending_dur = Rc::new(Cell::new(DurationFilter::AnyLength));
        let mut dur_btns = Vec::new();
        for (v, lbl) in dur_specs {
            let sel = v == DurationFilter::AnyLength;
            let btn = make_filter_item(&if sel { format!("{lbl} ✓") } else { lbl.to_string() }, sel);
            content.append(&btn);
            dur_btns.push((v, lbl, btn));
        }
        let dur_btns = Rc::new(dur_btns);
        for (v, _, btn) in dur_btns.iter() {
            let val = *v;
            let pd = pending_dur.clone();
            let all = dur_btns.clone();
            btn.connect_clicked(move |_| {
                pd.set(val);
                for (iv, il, b) in all.iter() {
                    let s = *iv == val;
                    update_filter_item(b, &if s { format!("{il} ✓") } else { il.to_string() }, s);
                }
            });
        }

        let sep2 = Separator::new(Orientation::Horizontal);
        sep2.set_margin_top(4);
        sep2.set_margin_bottom(4);
        content.append(&sep2);

        let h_sort = Label::new(Some("SORT BY"));
        h_sort.add_css_class("yr-popover-head");
        h_sort.set_halign(Align::Start);
        content.append(&h_sort);

        let sort_specs = [
            (SearchSortOrder::Relevance, "Relevance"),
            (SearchSortOrder::UploadDate, "Upload date"),
            (SearchSortOrder::ViewCount, "View count"),
        ];
        let pending_sort = Rc::new(Cell::new(SearchSortOrder::Relevance));
        let mut sort_btns = Vec::new();
        for (v, lbl) in sort_specs {
            let sel = v == SearchSortOrder::Relevance;
            let btn = make_filter_item(&if sel { format!("{lbl} ✓") } else { lbl.to_string() }, sel);
            content.append(&btn);
            sort_btns.push((v, lbl, btn));
        }
        let sort_btns = Rc::new(sort_btns);
        for (v, _, btn) in sort_btns.iter() {
            let val = *v;
            let ps = pending_sort.clone();
            let all = sort_btns.clone();
            btn.connect_clicked(move |_| {
                ps.set(val);
                for (iv, il, b) in all.iter() {
                    let s = *iv == val;
                    update_filter_item(b, &if s { format!("{il} ✓") } else { il.to_string() }, s);
                }
            });
        }

        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.set_homogeneous(true);
        footer.set_margin_top(8);
        footer.set_margin_start(6);
        footer.set_margin_end(6);
        footer.set_margin_bottom(4);

        let reset_btn = Button::with_label("Reset");
        reset_btn.add_css_class("yr-chip");
        let apply_btn = Button::with_label("Apply");
        apply_btn.add_css_class("yr-chip");
        apply_btn.add_css_class("active");

        footer.append(&reset_btn);
        footer.append(&apply_btn);
        content.append(&footer);

        popover.set_child(Some(&content));

        let weak_reset = Rc::downgrade(self);
        let pop_reset = popover.downgrade();
        let pd_r = pending_date.clone();
        let pdu_r = pending_dur.clone();
        let ps_r = pending_sort.clone();
        let db_r = date_btns.clone();
        let dub_r = dur_btns.clone();
        let sb_r = sort_btns.clone();
        reset_btn.connect_clicked(move |_| {
            pd_r.set(UploadDateFilter::AnyTime);
            pdu_r.set(DurationFilter::AnyLength);
            ps_r.set(SearchSortOrder::Relevance);
            for (iv, il, b) in db_r.iter() {
                let s = *iv == UploadDateFilter::AnyTime;
                update_filter_item(b, &if s { format!("{il} ✓") } else { il.to_string() }, s);
            }
            for (iv, il, b) in dub_r.iter() {
                let s = *iv == DurationFilter::AnyLength;
                update_filter_item(b, &if s { format!("{il} ✓") } else { il.to_string() }, s);
            }
            for (iv, il, b) in sb_r.iter() {
                let s = *iv == SearchSortOrder::Relevance;
                update_filter_item(b, &if s { format!("{il} ✓") } else { il.to_string() }, s);
            }
            if let Some(pop) = pop_reset.upgrade() {
                pop.popdown();
            }
            if let Some(w) = weak_reset.upgrade() {
                w.search_date_filter.set(UploadDateFilter::AnyTime);
                w.search_dur_filter.set(DurationFilter::AnyLength);
                w.search_sort_order.set(SearchSortOrder::Relevance);
                w.search_filters_btn.remove_css_class("active");
                let active_q = match &*w.active_view.borrow() {
                    ActiveView::Search(q) => Some(q.clone()),
                    _ => None,
                };
                if let Some(q) = active_q {
                    w.search(&q);
                }
            }
        });

        let weak_apply = Rc::downgrade(self);
        let pop_apply = popover.downgrade();
        apply_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_apply.upgrade() {
                pop.popdown();
            }
            if let Some(w) = weak_apply.upgrade() {
                let df = pending_date.get();
                let duf = pending_dur.get();
                let sf = pending_sort.get();
                w.search_date_filter.set(df);
                w.search_dur_filter.set(duf);
                w.search_sort_order.set(sf);
                if df != UploadDateFilter::AnyTime
                    || duf != DurationFilter::AnyLength
                    || sf != SearchSortOrder::Relevance
                {
                    w.search_filters_btn.add_css_class("active");
                } else {
                    w.search_filters_btn.remove_css_class("active");
                }
                let active_q = match &*w.active_view.borrow() {
                    ActiveView::Search(q) => Some(q.clone()),
                    _ => None,
                };
                if let Some(q) = active_q {
                    w.search(&q);
                }
            }
        });

        let pop_weak = popover.downgrade();
        self.search_filters_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popup();
            }
        });
    }

    pub fn make_card_actions(self: &Rc<Self>) -> CardActions {
        let settings_store = self.settings_store.clone();
        let default_quality: Rc<dyn Fn() -> Quality> = Rc::new(move || {
            let s = settings_store.load();
            Quality::from_setting(&s.default_quality)
        });

        let wl_check = self.watch_later_store.clone();
        let is_watch_later: Rc<dyn Fn(&str) -> bool> =
            Rc::new(move |vid_id: &str| wl_check.contains(vid_id));

        let weak = Rc::downgrade(self);
        let on_play: Rc<dyn Fn(VideoSummary, Quality)> = Rc::new(move |sum, q| {
            if let Some(w) = weak.upgrade() {
                w.play_video(sum, q);
            }
        });

        let weak = Rc::downgrade(self);
        let on_add_queue: Rc<dyn Fn(VideoSummary)> = Rc::new(move |sum| {
            if let Some(w) = weak.upgrade() {
                w.queue_video(sum);
            }
        });

        let weak = Rc::downgrade(self);
        let on_toggle_watch_later: Rc<dyn Fn(VideoSummary) -> bool> = Rc::new(move |sum| {
            let Some(w) = weak.upgrade() else {
                return false;
            };
            let saved = w.watch_later_store.toggle(sum);
            w.toast_overlay.add_toast(adw::Toast::new(if saved {
                "Saved to Watch Later"
            } else {
                "Removed from Watch Later"
            }));
            if *w.active_view.borrow() == ActiveView::WatchLater {
                w.load_watch_later();
            }
            saved
        });

        let weak = Rc::downgrade(self);
        let on_download: Rc<dyn Fn(VideoSummary, Quality)> = Rc::new(move |sum, q| {
            if let Some(w) = weak.upgrade() {
                w.start_download(sum, q);
            }
        });

        let weak = Rc::downgrade(self);
        let on_open_channel: Rc<dyn Fn(String, String)> = Rc::new(move |cid, cname| {
            if let Some(w) = weak.upgrade() {
                w.open_channel(&cid, &cname);
            }
        });

        let weak = Rc::downgrade(self);
        let on_toast: Rc<dyn Fn(String)> = Rc::new(move |msg| {
            if let Some(w) = weak.upgrade() {
                w.toast_overlay.add_toast(adw::Toast::new(&msg));
            }
        });

        CardActions {
            image_loader: self.image_loader.clone(),
            default_quality,
            is_watch_later,
            on_play,
            on_add_queue,
            on_toggle_watch_later,
            on_download,
            on_open_channel,
            on_toast,
        }
    }

    pub fn play_video(self: &Rc<Self>, summary: VideoSummary, quality: Quality) {
        let session = self.ensure_mpv_session();
        self.mpv_became_active.set(false);
        self.mpv_pos.set(0.0);
        self.mpv_duration
            .set(summary.duration_seconds.unwrap_or(0) as f64);

        let settings = self.settings_store.load();
        if settings.save_history {
            self.history_store.add(summary.clone());
        }

        let title = summary.title.clone();
        session.play(&summary.id, &title, quality);

        self.player_card
            .set_queue(vec![summary], 0, false);
        self.player_card.set_playing(true);

        let q_name = quality.label();
        self.toast_overlay.add_toast(adw::Toast::new(&format!(
            "Playing \"{title}\" ({q_name}) in MPV..."
        )));
    }

    pub fn play_shorts_queue(self: &Rc<Self>, shorts: Vec<VideoSummary>, start_index: usize) {
        if shorts.is_empty() {
            return;
        }
        let clamped_start = start_index.min(shorts.len() - 1);
        let session = self.ensure_mpv_session();
        self.mpv_became_active.set(false);
        self.mpv_pos.set(0.0);
        self.mpv_duration.set(
            shorts[clamped_start]
                .duration_seconds
                .unwrap_or(0) as f64,
        );

        let settings = self.settings_store.load();
        let mut quality = Quality::from_setting(&settings.default_quality);
        if quality == Quality::AudioOnly {
            quality = Quality::P360;
        }

        if settings.save_history {
            self.history_store.add(shorts[clamped_start].clone());
        }

        session.play(&shorts[0].id, &shorts[0].title, quality);
        for item in shorts.iter().skip(1) {
            session.append(&item.id);
        }
        if clamped_start > 0 {
            session.jump_to(clamped_start);
            session.set_media_title(&shorts[clamped_start].title);
        }

        let total = shorts.len();
        self.player_card.set_queue(shorts, clamped_start, true);
        self.player_card.set_playing(true);

        self.toast_overlay.add_toast(adw::Toast::new(&format!(
            "Playing Short {} of {total} in MPV...",
            clamped_start + 1
        )));
    }

    pub fn queue_video(self: &Rc<Self>, summary: VideoSummary) {
        if self.player_card.is_empty() {
            let def_q = Quality::from_setting(&self.settings_store.load().default_quality);
            self.play_video(summary, def_q);
            return;
        }

        let session = self.ensure_mpv_session();
        session.append(&summary.id);
        self.player_card.add_item(summary);
        self.toast_overlay
            .add_toast(adw::Toast::new("Added to queue"));
    }

    pub fn start_download(self: &Rc<Self>, summary: VideoSummary, quality: Quality) {
        let downloads_dir = glib::user_special_dir(glib::UserDirectory::Downloads)
            .unwrap_or_else(|| glib::home_dir().join("Downloads"));

        let short_title = summary.title.clone();
        self.toast_overlay.add_toast(adw::Toast::new(&format!(
            "Downloading \"{short_title}\" to Downloads..."
        )));

        let vid_id = summary.id.clone();
        let (tx, rx) = async_channel::bounded::<Result<std::path::PathBuf, String>>(1);
        self.runtime.spawn(async move {
            let res = download_with_ytdlp(&vid_id, quality, downloads_dir).await;
            let _ = tx.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else {
                return;
            };
            let Some(w) = weak.upgrade() else {
                return;
            };
            match res {
                Ok(_) => {
                    w.toast_overlay.add_toast(adw::Toast::new(&format!(
                        "Finished downloading \"{short_title}\""
                    )));
                }
                Err(err) => {
                    w.toast_overlay
                        .add_toast(adw::Toast::new(&format!("Download failed: {err}")));
                }
            }
        });
    }

    fn ensure_mpv_session(self: &Rc<Self>) -> Arc<MpvSession> {
        let settings = self.settings_store.load();

        // Reuse existing alive session unless user changed external player command
        {
            let mut slot = self.mpv_session.borrow_mut();
            if let Some(existing) = slot.as_ref() {
                let same_cmd = *self.mpv_player_cmd.borrow() == settings.external_player;
                if existing.is_alive() && same_cmd {
                    return existing.clone();
                }
                existing.shutdown();
                *slot = None;
            }
        }

        let (sender, receiver) = async_channel::unbounded::<MpvEvent>();
        let session = MpvSession::spawn(&self.runtime, &settings, sender);
        *self.mpv_player_cmd.borrow_mut() = settings.external_player.clone();
        *self.mpv_session.borrow_mut() = Some(session.clone());

        let weak = Rc::downgrade(self);
        let session_for_loop = session.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = receiver.recv().await {
                let Some(w) = weak.upgrade() else {
                    break;
                };
                // Ignore events from superseded sessions
                let is_current = w
                    .mpv_session
                    .borrow()
                    .as_ref()
                    .map(|cur| Arc::ptr_eq(cur, &session_for_loop))
                    .unwrap_or(false);
                if !is_current {
                    break;
                }

                match event {
                    MpvEvent::TimePos(pos) => {
                        w.mpv_became_active.set(true);
                        w.mpv_pos.set(pos);
                        let dur = w.mpv_duration.get();
                        w.player_card.set_progress(pos, dur);
                    }
                    MpvEvent::Duration(dur) => {
                        w.mpv_duration.set(dur);
                        let pos = w.mpv_pos.get();
                        w.player_card.set_progress(pos, dur);
                    }
                    MpvEvent::Pause(p) => {
                        w.player_card.set_playing(!p);
                    }
                    MpvEvent::PlaylistPos(idx) => {
                        w.player_card.set_current_index(idx);
                        if let Some(cur_vid) = w.player_card.current_video() {
                            session_for_loop.set_media_title(&cur_vid.title);
                            if w.settings_store.load().save_history {
                                w.history_store.add(cur_vid);
                            }
                        }
                    }
                    MpvEvent::Idle(idle) => {
                        if !idle {
                            w.mpv_became_active.set(true);
                        } else if w.mpv_became_active.get() {
                            w.player_card.set_playing(false);
                        }
                    }
                    MpvEvent::Error(msg) => {
                        w.toast_overlay.add_toast(adw::Toast::new(&msg));
                    }
                    MpvEvent::ProcessExited => {
                        *w.mpv_session.borrow_mut() = None;
                        w.player_card.set_queue(vec![], 0, false);
                        w.player_card.root.set_visible(false);
                        break;
                    }
                }
            }
        });

        session
    }

    pub fn open_channel(
        self: &Rc<Self>,
        channel_id: &str,
        channel_name: &str,
    ) -> Option<Rc<ChannelPage>> {
        if channel_id.is_empty() {
            return None;
        }

        let actions = self.make_card_actions();
        let weak_shorts = Rc::downgrade(self);
        let on_play_shorts: Rc<dyn Fn(Vec<VideoSummary>, usize)> =
            Rc::new(move |shorts, start_idx| {
                if let Some(w) = weak_shorts.upgrade() {
                    w.play_shorts_queue(shorts, start_idx);
                }
            });

        let channel_page = ChannelPage::new(
            channel_id,
            channel_name,
            &self.window,
            actions,
            self.engine.clone(),
            self.runtime.clone(),
            self.subscription_store.clone(),
            on_play_shorts,
            self.is_narrow.get(),
        );

        self.open_channels
            .borrow_mut()
            .push(channel_page.clone());

        self.nav_view.push(&channel_page.page);

        let (ch_tx, ch_rx) = async_channel::bounded::<
            Result<(crate::backend::models::ChannelDetails, Vec<VideoSummary>), String>,
        >(1);
        let (sh_tx, sh_rx) = async_channel::bounded::<Result<Vec<VideoSummary>, String>>(1);

        let engine = self.engine.clone();
        let cid = channel_id.to_string();
        self.runtime.spawn(async move {
            let res = engine.channel_videos(&cid).await;
            let _ = ch_tx.send(res).await;
        });

        let engine = self.engine.clone();
        let cid2 = channel_id.to_string();
        self.runtime.spawn(async move {
            let res = engine.channel_shorts(&cid2, None).await;
            let _ = sh_tx.send(res).await;
        });

        let cp_weak = Rc::downgrade(&channel_page);
        glib::spawn_future_local(async move {
            if let Ok(res) = ch_rx.recv().await {
                if let Some(cp) = cp_weak.upgrade() {
                    match res {
                        Ok((details, videos)) => {
                            cp.set_channel_details(details);
                            cp.set_videos(videos);
                        }
                        Err(err) => {
                            cp.set_load_error(&err);
                        }
                    }
                }
            }
        });

        let cp_weak = Rc::downgrade(&channel_page);
        glib::spawn_future_local(async move {
            if let Ok(Ok(shorts)) = sh_rx.recv().await {
                if let Some(cp) = cp_weak.upgrade() {
                    cp.set_shorts(shorts);
                }
            }
        });

        Some(channel_page)
    }

    fn next_request_id(&self) -> u64 {
        let next = self.request_seq.get().wrapping_add(1);
        self.request_seq.set(next);
        next
    }

    fn prepare_view_shell(
        &self,
        view: ActiveView,
        mode: ViewMode,
        show_chips: bool,
        show_subs: bool,
        show_history_header: bool,
        section_heading: Option<&str>,
    ) {
        self.nav_view.pop_to_page(&self.home_page);
        *self.active_view.borrow_mut() = view.clone();
        self.view_mode.set(mode);

        self.chips_scroll.set_visible(show_chips);
        self.subs_strip_box.set_visible(show_subs);
        self.history_header_box.set_visible(show_history_header);

        let is_search = matches!(view, ActiveView::Search(_));
        let is_wl = view == ActiveView::WatchLater;
        self.search_filters_btn.set_visible(is_search);
        self.clear_watch_later_btn.set_visible(is_wl);

        match section_heading {
            Some(txt) => {
                self.section_title.set_text(txt);
                self.section_header_row.set_visible(true);
            }
            None => {
                self.section_header_row.set_visible(false);
            }
        }

        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        self.update_flow_columns();
    }

    pub fn search(self: &Rc<Self>, query: &str) {
        let req_id = self.next_request_id();
        self.set_active_nav(RailTab::Home);
        self.prepare_view_shell(
            ActiveView::Search(query.to_string()),
            ViewMode::SearchList,
            false,
            false,
            false,
            Some(&format!("Results for \"{query}\"")),
        );
        self.show_skeletons_search(6);

        let (sender, receiver) = async_channel::bounded::<Result<Vec<VideoSummary>, String>>(1);
        let engine = self.engine.clone();
        let q = query.to_string();
        let q_ui = query.to_string();
        let df = self.search_date_filter.get();
        let duf = self.search_dur_filter.get();
        let sf = self.search_sort_order.get();

        self.runtime.spawn(async move {
            let res = engine.search_filtered(&q, df, duf, sf).await;
            let _ = sender.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = receiver.recv().await else {
                return;
            };
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.request_seq.get() != req_id {
                return;
            }
            match res {
                Ok(videos) => {
                    if videos.is_empty() {
                        let weak_clear = Rc::downgrade(&w);
                        w.show_status_state(
                            "system-search-symbolic",
                            "No results",
                            &format!(
                                "Nothing matched \"{q_ui}\". Check the spelling or try different words."
                            ),
                            Some(("Clear search", Rc::new(move || {
                                if let Some(win) = weak_clear.upgrade() {
                                    win.search_entry.set_text("");
                                    win.highlight_chip(Some(&win.chip_trending));
                                    win.load_trending();
                                }
                            }))),
                        );
                    } else {
                        w.populate_videos(videos, ViewMode::SearchList);
                    }
                }
                Err(e) => {
                    let weak_retry = Rc::downgrade(&w);
                    let q_retry = q_ui.clone();
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Could not load search results",
                        &e,
                        Some(("Retry", Rc::new(move || {
                            if let Some(win) = weak_retry.upgrade() {
                                win.search(&q_retry);
                            }
                        }))),
                    );
                }
            }
        });
    }

    pub fn search_category(self: &Rc<Self>, category: &str) {
        let req_id = self.next_request_id();
        self.prepare_view_shell(
            ActiveView::HomeCategory(category.to_string()),
            ViewMode::Grid,
            true,
            false,
            false,
            Some(category),
        );
        self.show_skeletons(8);

        let (sender, receiver) = async_channel::bounded::<Result<Vec<VideoSummary>, String>>(1);
        let engine = self.engine.clone();
        let cat = category.to_string();
        let cat_ui = category.to_string();

        self.runtime.spawn(async move {
            let res = engine.search(&cat).await;
            let _ = sender.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = receiver.recv().await else {
                return;
            };
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.request_seq.get() != req_id {
                return;
            }
            match res {
                Ok(videos) => {
                    if videos.is_empty() {
                        w.show_status_state(
                            "system-search-symbolic",
                            "No videos found",
                            "Please try again in a moment.",
                            None,
                        );
                    } else {
                        w.populate_videos(videos, ViewMode::Grid);
                    }
                }
                Err(e) => {
                    let weak_retry = Rc::downgrade(&w);
                    let cat_retry = cat_ui.clone();
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Network Error",
                        &e,
                        Some(("Retry", Rc::new(move || {
                            if let Some(win) = weak_retry.upgrade() {
                                win.search_category(&cat_retry);
                            }
                        }))),
                    );
                }
            }
        });
    }

    pub fn load_trending(self: &Rc<Self>) {
        let req_id = self.next_request_id();
        self.prepare_view_shell(
            ActiveView::HomeTrending,
            ViewMode::Grid,
            true,
            false,
            false,
            Some("Trending"),
        );
        self.show_skeletons(8);

        let (sender, receiver) = async_channel::bounded::<Result<Vec<VideoSummary>, String>>(1);
        let engine = self.engine.clone();

        self.runtime.spawn(async move {
            let res = engine.trending().await;
            let _ = sender.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = receiver.recv().await else {
                return;
            };
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.request_seq.get() != req_id {
                return;
            }
            match res {
                Ok(videos) => {
                    if videos.is_empty() {
                        let weak_retry = Rc::downgrade(&w);
                        w.show_status_state(
                            "system-search-symbolic",
                            "No trending videos",
                            "Check back again shortly.",
                            Some(("Retry", Rc::new(move || {
                                if let Some(win) = weak_retry.upgrade() {
                                    win.load_trending();
                                }
                            }))),
                        );
                    } else {
                        w.populate_videos(videos, ViewMode::Grid);
                    }
                }
                Err(e) => {
                    let weak_retry = Rc::downgrade(&w);
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Network Error",
                        &e,
                        Some(("Retry", Rc::new(move || {
                            if let Some(win) = weak_retry.upgrade() {
                                win.load_trending();
                            }
                        }))),
                    );
                }
            }
        });
    }

    pub fn load_watch_later(self: &Rc<Self>) {
        let _req_id = self.next_request_id();
        let saved = self.watch_later_store.load_all();
        self.prepare_view_shell(
            ActiveView::WatchLater,
            ViewMode::Grid,
            true,
            false,
            false,
            Some("Watch Later"),
        );
        self.clear_watch_later_btn.set_visible(!saved.is_empty());

        if saved.is_empty() {
            self.show_status_state(
                "bookmark-new-symbolic",
                "Watch Later is empty",
                "Click \"Watch later\" on any video to save it here for later.",
                None,
            );
            return;
        }

        self.populate_videos(saved, ViewMode::Grid);
    }

    pub fn load_subscriptions(self: &Rc<Self>) {
        let req_id = self.next_request_id();
        let subs = self.subscription_store.load_all();

        if subs.is_empty() {
            self.prepare_view_shell(
                ActiveView::Subscriptions,
                ViewMode::Grid,
                false,
                false,
                false,
                Some("Subscriptions"),
            );
            let weak_home = Rc::downgrade(self);
            self.show_status_state(
                "emblem-favorite-symbolic",
                "No Subscriptions Yet",
                "Open any channel and click Subscribe to see their latest uploads here.",
                Some(("Browse Trending", Rc::new(move || {
                    if let Some(win) = weak_home.upgrade() {
                        win.set_active_nav(RailTab::Home);
                        win.highlight_chip(Some(&win.chip_trending));
                        win.load_trending();
                    }
                }))),
            );
            return;
        }

        self.prepare_view_shell(
            ActiveView::Subscriptions,
            ViewMode::Grid,
            false,
            true,
            false,
            Some("Latest"),
        );

        while let Some(child) = self.subs_avatars_row.first_child() {
            self.subs_avatars_row.remove(&child);
        }

        for sub in &subs {
            let item_box = GtkBox::new(Orientation::Vertical, 6);
            item_box.add_css_class("yr-avatar-btn");
            item_box.set_valign(Align::Center);
            item_box.set_tooltip_text(Some(&sub.name));

            let av = adw::Avatar::new(56, Some(&sub.name), true);
            av.set_valign(Align::Center);
            if !sub.avatar_url.is_empty() {
                self.image_loader
                    .load_into_avatar(&sub.id, &sub.avatar_url, &av);
            }

            let lbl = Label::new(Some(&sub.name));
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            lbl.set_max_width_chars(11);
            lbl.add_css_class("yr-meta");

            item_box.append(&av);
            item_box.append(&lbl);

            let click = GestureClick::new();
            click.set_button(1);
            let cid = sub.id.clone();
            let cname = sub.name.clone();
            let weak_chan = Rc::downgrade(self);
            click.connect_released(move |_, _, _, _| {
                if let Some(w) = weak_chan.upgrade() {
                    w.open_channel(&cid, &cname);
                }
            });
            item_box.add_controller(click);

            self.subs_avatars_row.append(&item_box);
        }

        self.show_skeletons(8);

        let (sender, receiver) = async_channel::bounded::<Result<Vec<VideoSummary>, String>>(1);
        let engine = self.engine.clone();
        self.runtime.spawn(async move {
            let res = engine.subscriptions_feed(subs).await;
            let _ = sender.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = receiver.recv().await else {
                return;
            };
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.request_seq.get() != req_id {
                return;
            }
            match res {
                Ok(videos) => {
                    if videos.is_empty() {
                        let weak_retry = Rc::downgrade(&w);
                        w.show_status_state(
                            "emblem-favorite-symbolic",
                            "No Recent Uploads",
                            "Could not find recent videos from your subscribed channels.",
                            Some(("Refresh", Rc::new(move || {
                                if let Some(win) = weak_retry.upgrade() {
                                    win.load_subscriptions();
                                }
                            }))),
                        );
                    } else {
                        w.populate_videos(videos, ViewMode::Grid);
                    }
                }
                Err(err) => {
                    let weak_retry = Rc::downgrade(&w);
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Could not load subscriptions",
                        &err,
                        Some(("Retry", Rc::new(move || {
                            if let Some(win) = weak_retry.upgrade() {
                                win.load_subscriptions();
                            }
                        }))),
                    );
                }
            }
        });
    }

    pub fn load_history(self: &Rc<Self>) {
        let _req_id = self.next_request_id();
        self.prepare_view_shell(
            ActiveView::History,
            ViewMode::SearchList,
            false,
            false,
            true,
            None,
        );

        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        let entries = self.history_store.load_all();
        if entries.is_empty() {
            self.show_status_state(
                "document-open-recent-symbolic",
                "No Watch History",
                "Videos you play in MPV will appear here.",
                None,
            );
            return;
        }

        let actions = self.make_card_actions();

        for entry in entries {
            let weak_rm = Rc::downgrade(self);
            let vid = entry.video.id.clone();
            let card = VideoCard::new_history(&entry, move || {
                if let Some(w) = weak_rm.upgrade() {
                    w.history_store.remove(&vid);
                    w.load_history();
                }
            });

            self.image_loader.load_into_picture(
                &entry.video.id,
                &entry.video.thumbnail_url,
                &card.picture,
            );

            let weak_click = Rc::downgrade(self);
            let sum_cl = entry.video.clone();
            let act_cl = actions.clone();
            card.connect_primary_click(move || {
                if let Some(w) = weak_click.upgrade() {
                    DetailsSheet::show(&w.window, &sum_cl, &act_cl);
                }
            });

            ContextMenu::attach(&card.container, &entry.video, &actions);
            self.flow_box.append(&card.container);
        }
    }

    pub fn open_settings(self: &Rc<Self>) {
        let prev_view = self.active_view.borrow().clone();
        self.set_active_nav(RailTab::Settings);

        let prefs = adw::PreferencesDialog::builder()
            .title("Preferences")
            .build();

        let weak_closed = Rc::downgrade(self);
        prefs.connect_closed(move |_| {
            if let Some(w) = weak_closed.upgrade() {
                let restore_tab = match prev_view {
                    ActiveView::Subscriptions => RailTab::Subscriptions,
                    ActiveView::History => RailTab::History,
                    _ => RailTab::Home,
                };
                w.set_active_nav(restore_tab);
            }
        });

        let page = adw::PreferencesPage::builder()
            .title("Preferences")
            .icon_name("emblem-system-symbolic")
            .build();

        let settings = self.settings_store.load();

        // --- Group 1: Appearance (matches Settings.png) ---
        let group_appearance = adw::PreferencesGroup::builder()
            .title("Appearance")
            .build();

        let scheme_model = gtk4::StringList::new(&["Dark", "Follow system", "Light"]);
        let scheme_idx = match settings.color_scheme.as_str() {
            "system" => 1,
            "light" => 2,
            _ => 0,
        };
        let scheme_row = adw::ComboRow::builder()
            .title("Color scheme")
            .subtitle("Follow the system or force a mode")
            .model(&scheme_model)
            .selected(scheme_idx)
            .build();

        let weak_scheme = Rc::downgrade(self);
        scheme_row.connect_selected_notify(move |row| {
            if let Some(w) = weak_scheme.upgrade() {
                let mut s = w.settings_store.load();
                s.color_scheme = match row.selected() {
                    1 => "system".to_string(),
                    2 => "light".to_string(),
                    _ => "dark".to_string(),
                };
                w.settings_store.save(&s);
                w.apply_appearance(&s);
            }
        });

        let accent_model = gtk4::StringList::new(&["Terracotta", "Forest", "Plum", "Teal"]);
        let accent_idx = match settings.accent_color.as_str() {
            "forest" => 1,
            "plum" => 2,
            "teal" => 3,
            _ => 0,
        };
        let accent_row = adw::ComboRow::builder()
            .title("Accent color")
            .subtitle("Terracotta, Forest, Plum or Teal")
            .model(&accent_model)
            .selected(accent_idx)
            .build();

        let weak_accent = Rc::downgrade(self);
        accent_row.connect_selected_notify(move |row| {
            if let Some(w) = weak_accent.upgrade() {
                let mut s = w.settings_store.load();
                s.accent_color = match row.selected() {
                    1 => "forest".to_string(),
                    2 => "plum".to_string(),
                    3 => "teal".to_string(),
                    _ => "terracotta".to_string(),
                };
                w.settings_store.save(&s);
                w.apply_appearance(&s);
            }
        });

        group_appearance.add(&scheme_row);
        group_appearance.add(&accent_row);

        // --- Group 2: Playback ---
        let group_playback = adw::PreferencesGroup::builder()
            .title("Playback")
            .build();

        let quality_model = gtk4::StringList::new(&["360p", "720p", "1080p", "Audio only"]);
        let selected_idx = match Quality::from_setting(&settings.default_quality) {
            Quality::P360 => 0,
            Quality::P720 => 1,
            Quality::P1080 => 2,
            Quality::AudioOnly => 3,
        };
        let quality_row = adw::ComboRow::builder()
            .title("Default quality")
            .subtitle("Quality the video starts with")
            .model(&quality_model)
            .selected(selected_idx)
            .build();

        let store_q = self.settings_store.clone();
        quality_row.connect_selected_notify(move |row| {
            let mut s = store_q.load();
            let chosen = match row.selected() {
                1 => Quality::P720,
                2 => Quality::P1080,
                3 => Quality::AudioOnly,
                _ => Quality::P360,
            };
            s.default_quality = chosen.as_setting().to_string();
            store_q.save(&s);
        });

        let hwdec_row = adw::SwitchRow::builder()
            .title("Hardware decoding")
            .subtitle("Use VA-API when available")
            .active(settings.hardware_decoding)
            .build();

        let weak_hw = Rc::downgrade(self);
        hwdec_row.connect_active_notify(move |row| {
            if let Some(w) = weak_hw.upgrade() {
                let active = row.is_active();
                let mut s = w.settings_store.load();
                s.hardware_decoding = active;
                w.settings_store.save(&s);
                if let Some(sess) = w.mpv_session.borrow().as_ref() {
                    sess.set_hardware_decoding(active);
                }
            }
        });

        let player_row = adw::EntryRow::builder()
            .title("Player command")
            .text(&settings.external_player)
            .build();

        let store_pl = self.settings_store.clone();
        player_row.connect_changed(move |row| {
            let mut s = store_pl.load();
            let txt = row.text().trim().to_string();
            s.external_player = if txt.is_empty() {
                "mpv".to_string()
            } else {
                txt
            };
            store_pl.save(&s);
        });

        group_playback.add(&quality_row);
        group_playback.add(&hwdec_row);
        group_playback.add(&player_row);

        // --- Group 3: Data ---
        let group_data = adw::PreferencesGroup::builder()
            .title("Data")
            .build();

        let clear_cache_row = adw::ActionRow::builder()
            .title("Clear thumbnail cache")
            .subtitle("Free disk space used by images")
            .build();
        let clear_cache_btn = Button::with_label("Clear");
        clear_cache_btn.add_css_class("yr-btn");
        clear_cache_btn.set_valign(Align::Center);
        let weak_cache = Rc::downgrade(self);
        clear_cache_btn.connect_clicked(move |_| {
            if let Some(w) = weak_cache.upgrade() {
                w.image_loader.clear_all();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Thumbnail cache cleared"));
            }
        });
        clear_cache_row.add_suffix(&clear_cache_btn);

        let hist_row = adw::SwitchRow::builder()
            .title("Save watch history")
            .subtitle("Stored locally on this device")
            .active(settings.save_history)
            .build();

        let store_h = self.settings_store.clone();
        hist_row.connect_active_notify(move |row| {
            let mut s = store_h.load();
            s.save_history = row.is_active();
            store_h.save(&s);
        });

        let clear_hist_row = adw::ActionRow::builder()
            .title("Clear watch history")
            .subtitle("Deletes all recorded watch history")
            .build();
        let clear_hist_btn = Button::with_label("Clear");
        clear_hist_btn.add_css_class("yr-btn");
        clear_hist_btn.set_valign(Align::Center);
        let weak_hist = Rc::downgrade(self);
        clear_hist_btn.connect_clicked(move |_| {
            if let Some(w) = weak_hist.upgrade() {
                w.history_store.clear();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Watch history cleared"));
                if *w.active_view.borrow() == ActiveView::History {
                    w.load_history();
                }
            }
        });
        clear_hist_row.add_suffix(&clear_hist_btn);

        group_data.add(&clear_cache_row);
        group_data.add(&hist_row);
        group_data.add(&clear_hist_row);

        page.add(&group_appearance);
        page.add(&group_playback);
        page.add(&group_data);
        prefs.add(&page);
        prefs.present(Some(&self.window));
    }

    fn show_shortcuts_dialog(&self) {
        let dialog = adw::Dialog::builder()
            .content_width(420)
            .title("Keyboard Shortcuts")
            .build();
        dialog.add_css_class("yr-sheet");

        let vbox = GtkBox::new(Orientation::Vertical, 12);
        vbox.set_margin_top(20);
        vbox.set_margin_bottom(24);
        vbox.set_margin_start(24);
        vbox.set_margin_end(24);

        let heading = Label::new(Some("Keyboard Shortcuts"));
        heading.add_css_class("yr-sheet-title");
        heading.set_halign(Align::Start);
        vbox.append(&heading);

        let items = [
            ("/ or Ctrl + F", "Focus the search bar"),
            ("Enter", "Run search"),
            ("Esc", "Close dialog / unfocus search"),
            ("Alt + Left", "Go back from Channel page"),
            ("Space", "Pause or resume when Player Card is focused"),
        ];

        for (keys, desc) in items {
            let row = GtkBox::new(Orientation::Horizontal, 12);
            let desc_lbl = Label::new(Some(desc));
            desc_lbl.add_css_class("yr-meta");
            desc_lbl.set_hexpand(true);
            desc_lbl.set_halign(Align::Start);

            let key_lbl = Label::new(Some(keys));
            key_lbl.add_css_class("yr-chip");

            row.append(&desc_lbl);
            row.append(&key_lbl);
            vbox.append(&row);
        }

        let close_btn = Button::with_label("Close");
        close_btn.add_css_class("yr-btn");
        close_btn.set_halign(Align::End);
        close_btn.set_margin_top(8);
        let dlg_weak = dialog.downgrade();
        close_btn.connect_clicked(move |_| {
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
        vbox.append(&close_btn);

        dialog.set_child(Some(&vbox));
        dialog.present(Some(&self.window));
    }

    fn show_about_dialog(&self) {
        let dialog = adw::Dialog::builder()
            .content_width(400)
            .title("About youtube-rusty")
            .build();
        dialog.add_css_class("yr-sheet");

        let vbox = GtkBox::new(Orientation::Vertical, 10);
        vbox.set_margin_top(24);
        vbox.set_margin_bottom(24);
        vbox.set_margin_start(24);
        vbox.set_margin_end(24);
        vbox.set_halign(Align::Center);

        let logo = Button::from_icon_name("media-playback-start-symbolic");
        logo.add_css_class("yr-logo");
        logo.set_halign(Align::Center);
        logo.set_sensitive(false);
        vbox.append(&logo);

        let name = Label::new(Some("youtube-rusty"));
        name.add_css_class("yr-h1");
        vbox.append(&name);

        let ver = Label::new(Some("Version 0.1.0"));
        ver.add_css_class("yr-meta");
        vbox.append(&ver);

        let desc = Label::new(Some(
            "Lightweight Linux desktop YouTube client built with Rust, GTK4, Libadwaita, RustyPipe, and MPV.",
        ));
        desc.add_css_class("yr-meta");
        desc.set_wrap(true);
        desc.set_justify(gtk4::Justification::Center);
        desc.set_max_width_chars(40);
        vbox.append(&desc);

        let close_btn = Button::with_label("Close");
        close_btn.add_css_class("yr-btn");
        close_btn.add_css_class("primary");
        close_btn.set_halign(Align::Center);
        close_btn.set_margin_top(8);
        let dlg_weak = dialog.downgrade();
        close_btn.connect_clicked(move |_| {
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });
        vbox.append(&close_btn);

        dialog.set_child(Some(&vbox));
        dialog.present(Some(&self.window));
    }

    fn show_skeletons(self: &Rc<Self>, count: usize) {
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.view_mode.set(ViewMode::Grid);
        self.update_flow_columns();
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_skeleton());
        }
    }

    fn show_skeletons_search(self: &Rc<Self>, count: usize) {
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.view_mode.set(ViewMode::SearchList);
        self.update_flow_columns();
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_search_skeleton());
        }
    }

    fn show_status_state(
        &self,
        icon_name: &str,
        title: &str,
        description: &str,
        action: Option<(&str, Rc<dyn Fn()>)>,
    ) {
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.flow_box.set_visible(false);

        self.status_icon.set_icon_name(Some(icon_name));
        self.status_title.set_text(title);
        self.status_desc.set_text(description);

        match action {
            Some((label, cb)) => {
                self.status_action_btn.set_label(label);
                self.status_action_btn.set_visible(true);
                *self.status_action_cb.borrow_mut() = Some(cb);
            }
            None => {
                self.status_action_btn.set_visible(false);
                *self.status_action_cb.borrow_mut() = None;
            }
        }

        self.status_box.set_visible(true);
    }

    pub fn populate_videos(self: &Rc<Self>, videos: Vec<VideoSummary>, mode: ViewMode) {
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);

        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        self.view_mode.set(mode);
        self.update_flow_columns();

        let actions = self.make_card_actions();

        for summary in videos {
            let card = match mode {
                ViewMode::Grid => VideoCard::new_grid(&summary),
                ViewMode::SearchList => VideoCard::new_search(&summary),
            };

            let weak_click = Rc::downgrade(self);
            let sum_cl = summary.clone();
            let act_cl = actions.clone();
            card.connect_primary_click(move || {
                if let Some(w) = weak_click.upgrade() {
                    DetailsSheet::show(&w.window, &sum_cl, &act_cl);
                }
            });

            ContextMenu::attach(&card.container, &summary, &actions);

            self.image_loader
                .load_into_picture(&summary.id, &summary.thumbnail_url, &card.picture);
            if !summary.channel_avatar_url.is_empty() {
                self.image_loader.load_into_avatar(
                    &summary.channel_id,
                    &summary.channel_avatar_url,
                    &card.avatar,
                );
            }

            self.flow_box.append(&card.container);
        }
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

fn load_custom_css() -> CssProvider {
    let base_provider = CssProvider::new();
    let css = include_str!("style.css");
    base_provider.load_from_string(css);

    let theme_provider = CssProvider::new();

    if let Some(display) = Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &base_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk4::style_context_add_provider_for_display(
            &display,
            &theme_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }

    theme_provider
}

fn create_bottom_nav_btn(icon_name: &str, tooltip: &str, is_active: bool) -> Button {
    let btn = Button::from_icon_name(icon_name);
    btn.add_css_class("yr-rail-btn");
    btn.set_tooltip_text(Some(tooltip));
    btn.set_hexpand(true);
    btn.set_halign(Align::Center);
    btn.set_width_request(44);
    btn.set_height_request(44);
    if is_active {
        btn.add_css_class("active");
    }
    btn
}

fn make_filter_item(label: &str, selected: bool) -> Button {
    let btn = Button::builder().label(label).halign(Align::Fill).build();
    btn.add_css_class("flat");
    btn.add_css_class("yr-menu-item");
    if selected {
        btn.add_css_class("selected");
    }
    if let Some(child) = btn.child() {
        if let Some(lbl) = child.downcast_ref::<Label>() {
            lbl.set_halign(Align::Start);
            lbl.set_xalign(0.0);
        }
    }
    btn
}

fn update_filter_item(btn: &Button, label: &str, selected: bool) {
    btn.set_label(label);
    if selected {
        btn.add_css_class("selected");
    } else {
        btn.remove_css_class("selected");
    }
    if let Some(child) = btn.child() {
        if let Some(lbl) = child.downcast_ref::<Label>() {
            lbl.set_halign(Align::Start);
            lbl.set_xalign(0.0);
        }
    }
}
