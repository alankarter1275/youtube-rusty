use crate::backend::cache::Cache;
use crate::backend::engine::{download_with_ytdlp, Engine, PageCursor, PagedVideos};
use crate::backend::history::HistoryStore;
use crate::backend::models::{
    ChannelDetails, DurationFilter, SearchSortOrder, UploadDateFilter, VideoSummary,
};
use crate::backend::settings::{AppSettings, SettingsStore};
use crate::backend::subscriptions::SubscriptionStore;
use crate::backend::watch_later::WatchLaterStore;
use crate::player::mpv::Quality;
use crate::player::mpv_ipc::{MpvEvent, MpvSession};
use crate::ui::channel_page::ChannelPage;
use crate::ui::details_sheet::{CardActions, ContextMenu, DetailsSheet};
use crate::ui::icons::{make_icon, set_pointer, KalamIcon};
use crate::ui::image_loader::ImageLoader;
use crate::ui::player_card::PlayerCard;
use crate::ui::rail::{Rail, RailTab};
use crate::ui::video_card::{ListCardLayout, VideoCard};
use adw::prelude::*;
use gtk4::gdk::{self, Display};
use gtk4::{
    gio, glib, Align, Box as GtkBox, Button, CssProvider, Editable, Entry, EventControllerKey,
    FlowBox, GestureClick, Image, Label, MenuButton, Orientation, Overlay, Popover, ScrolledWindow,
    SearchEntry, SelectionMode, Separator, Switch,
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
    Preferences,
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    back_btn: Button,
    pub search_entry: SearchEntry,
    suppress_search_clear: Cell<bool>,
    pub rail: Rail,
    bottom_home: Button,
    bottom_subs: Button,
    bottom_hist: Button,
    bottom_settings: Button,
    pub nav_view: adw::NavigationView,
    pub home_page: adw::NavigationPage,
    content_scrolled: ScrolledWindow,
    feed_box: GtkBox,
    prefs_clamp: adw::Clamp,
    pref_scheme_btn: Button,
    pref_accent_btn: Button,
    pref_quality_btn: Button,
    pref_hwdec_switch: Switch,
    pref_player_entry: Entry,
    pref_hist_switch: Switch,
    pub flow_box: FlowBox,
    load_more_btn: Button,
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
    chips_box: GtkBox,
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
    is_compact: Cell<bool>,
    grid_cols: Cell<u32>,
    shorts_cols: Cell<u32>,
    view_mode: Cell<ViewMode>,
    active_view: RefCell<ActiveView>,
    prev_non_search_view: RefCell<ActiveView>,
    current_videos: RefCell<Vec<VideoSummary>>,
    feed_cursor: RefCell<Option<PageCursor>>,
    is_loading_more: Cell<bool>,
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

        // HeaderBar: borderless, no window close button, centered search bar
        let header_bar = adw::HeaderBar::new();
        header_bar.set_show_end_title_buttons(false);
        header_bar.set_show_start_title_buttons(false);

        let back_btn = Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Back (Alt+Left)")
            .visible(false)
            .build();
        back_btn.add_css_class("flat");
        back_btn.add_css_class("circular");
        set_pointer(&back_btn);
        header_bar.pack_start(&back_btn);

        // Centered Search Bar: max 420px, shrinks smoothly on narrow windows
        let search_clamp = adw::Clamp::builder()
            .maximum_size(420)
            .tightening_threshold(240)
            .hexpand(true)
            .build();
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search YouTube")
            .width_request(120)
            .hexpand(true)
            .build();
        search_entry.add_css_class("yr-search");
        search_clamp.set_child(Some(&search_entry));
        header_bar.set_title_widget(Some(&search_clamp));

        // Primary Menu Button at the top right (exact 3-bar Kalam icon)
        let menu_btn = MenuButton::new();
        menu_btn.set_child(Some(&make_icon(KalamIcon::Menu, 22)));
        menu_btn.set_tooltip_text(Some("Main menu"));
        menu_btn.add_css_class("yr-rail-btn");
        set_pointer(&menu_btn);

        let menu = gio::Menu::new();
        menu.append(Some("Preferences"), Some("app.settings"));
        menu.append(Some("Watch Later"), Some("app.watch_later"));
        menu.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        menu.append(Some("About youtube-rusty"), Some("app.about"));
        menu_btn.set_menu_model(Some(&menu));
        header_bar.pack_end(&menu_btn);

        // Full-height Left Navigation Rail (68px, spans entire window height)
        let rail = Rail::new();

        // Scrollable Chips Row
        let chips_box = GtkBox::new(Orientation::Horizontal, 8);
        chips_box.set_margin_start(28);
        chips_box.set_margin_end(28);
        chips_box.set_margin_top(10);
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
        subs_strip_box.set_margin_top(12);
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
        history_header_box.set_margin_top(12);
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
        set_pointer(&clear_hist_btn);

        history_header_box.append(&hist_title);
        history_header_box.append(&clear_hist_btn);

        // Section Header Row (Title + Search Filters / Clear Watch Later)
        let section_header_row = GtkBox::new(Orientation::Horizontal, 12);
        section_header_row.set_margin_start(28);
        section_header_row.set_margin_end(28);
        section_header_row.set_margin_top(4);
        section_header_row.set_margin_bottom(14);
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
        set_pointer(&search_filters_btn);

        let clear_watch_later_btn = Button::with_label("Clear saved");
        clear_watch_later_btn.add_css_class("yr-chip");
        clear_watch_later_btn.set_visible(false);
        set_pointer(&clear_watch_later_btn);

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
        status_desc.set_max_width_chars(46);

        let status_action_btn = Button::with_label("Retry");
        status_action_btn.add_css_class("yr-btn");
        status_action_btn.add_css_class("primary");
        status_action_btn.set_halign(Align::Center);
        status_action_btn.set_margin_top(6);
        set_pointer(&status_action_btn);

        status_box.append(&icon_circle);
        status_box.append(&status_title);
        status_box.append(&status_desc);
        status_box.append(&status_action_btn);

        // Adaptive Video Grid
        let flow_box = FlowBox::new();
        flow_box.set_valign(Align::Start);
        flow_box.set_max_children_per_line(4);
        flow_box.set_min_children_per_line(4);
        flow_box.set_selection_mode(SelectionMode::None);
        flow_box.set_column_spacing(20);
        flow_box.set_row_spacing(24);
        flow_box.set_homogeneous(true);
        flow_box.set_margin_start(28);
        flow_box.set_margin_end(28);
        flow_box.set_margin_bottom(16);

        let load_more_btn = Button::with_label("Load more");
        load_more_btn.add_css_class("yr-btn");
        load_more_btn.set_halign(Align::Center);
        load_more_btn.set_margin_top(4);
        load_more_btn.set_margin_bottom(36);
        load_more_btn.set_visible(false);
        set_pointer(&load_more_btn);

        let feed_box = GtkBox::new(Orientation::Vertical, 0);
        feed_box.set_vexpand(true);
        feed_box.append(&chips_scroll);
        feed_box.append(&subs_strip_box);
        feed_box.append(&history_header_box);
        feed_box.append(&section_header_row);
        feed_box.append(&status_box);
        feed_box.append(&flow_box);
        feed_box.append(&load_more_btn);

        // Inline Kalam Preferences View (matches Settings.png 100%)
        let (
            prefs_clamp,
            pref_scheme_btn,
            pref_accent_btn,
            pref_quality_btn,
            pref_hwdec_switch,
            pref_player_entry,
            pref_hist_switch,
            pref_clear_cache_btn,
            pref_clear_hist_btn,
        ) = build_inline_preferences_view();

        let content_v_box = GtkBox::new(Orientation::Vertical, 0);
        content_v_box.set_vexpand(true);
        content_v_box.set_hexpand(true);
        content_v_box.append(&feed_box);
        content_v_box.append(&prefs_clamp);

        let content_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&content_v_box)
            .vexpand(true)
            .hexpand(true)
            .build();

        let nav_view = adw::NavigationView::new();
        nav_view.set_hexpand(true);
        nav_view.set_vexpand(true);
        let home_page = adw::NavigationPage::builder()
            .title("Home")
            .tag("home")
            .child(&content_scrolled)
            .build();
        nav_view.push(&home_page);

        let cache = Arc::new(Cache::new());
        let runtime = Arc::new(Runtime::new().expect("Failed to initialize Tokio runtime"));
        let engine = Arc::new(Engine::new(cache.root_dir()));
        let image_loader = ImageLoader::new(cache, runtime.clone());

        let content_overlay = Overlay::new();
        content_overlay.set_hexpand(true);
        content_overlay.set_vexpand(true);
        content_overlay.set_child(Some(&nav_view));

        let player_card = PlayerCard::new(image_loader.clone());
        content_overlay.add_overlay(&player_card.root);

        // Bottom Navigation Bar for Narrow Breakpoint (<= 600px)
        let bottom_bar = GtkBox::new(Orientation::Horizontal, 0);
        bottom_bar.add_css_class("yr-bottom-bar");
        bottom_bar.set_visible(false);

        let bottom_home = create_bottom_nav_btn(KalamIcon::Home, "Home", true);
        let bottom_subs = create_bottom_nav_btn(KalamIcon::Subscriptions, "Subscriptions", false);
        let bottom_hist = create_bottom_nav_btn(KalamIcon::History, "History", false);
        let bottom_settings = create_bottom_nav_btn(KalamIcon::Settings, "Preferences", false);

        bottom_bar.append(&bottom_home);
        bottom_bar.append(&bottom_subs);
        bottom_bar.append(&bottom_hist);
        bottom_bar.append(&bottom_settings);

        // Right column: HeaderBar + Content + Narrow Bottom Bar
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.set_hexpand(true);
        toolbar_view.set_vexpand(true);
        toolbar_view.add_top_bar(&header_bar);
        toolbar_view.add_bottom_bar(&bottom_bar);
        toolbar_view.set_content(Some(&content_overlay));

        // Outer shell: Full-height Siderail on the left, ToolbarView on the right!
        let shell_h_box = GtkBox::new(Orientation::Horizontal, 0);
        shell_h_box.set_hexpand(true);
        shell_h_box.set_vexpand(true);
        shell_h_box.append(&rail.container);
        shell_h_box.append(&toolbar_view);

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&shell_h_box));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("youtube-rusty")
            .default_width(1240)
            .default_height(780)
            .width_request(340)
            .height_request(420)
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
            suppress_search_clear: Cell::new(false),
            rail,
            bottom_home,
            bottom_subs,
            bottom_hist,
            bottom_settings,
            nav_view,
            home_page,
            content_scrolled,
            feed_box,
            prefs_clamp,
            pref_scheme_btn,
            pref_accent_btn,
            pref_quality_btn,
            pref_hwdec_switch,
            pref_player_entry,
            pref_hist_switch,
            flow_box,
            load_more_btn,
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
            chips_box,
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
            is_compact: Cell::new(false),
            grid_cols: Cell::new(4),
            shorts_cols: Cell::new(6),
            view_mode: Cell::new(ViewMode::Grid),
            active_view: RefCell::new(ActiveView::HomeTrending),
            prev_non_search_view: RefCell::new(ActiveView::HomeTrending),
            current_videos: RefCell::new(Vec::new()),
            feed_cursor: RefCell::new(None),
            is_loading_more: Cell::new(false),
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
        this.sync_preferences_ui(&initial_settings);
        this.wire_preferences_signals(&pref_clear_cache_btn, &pref_clear_hist_btn);
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
                 .yr-card:hover .yr-thumb, .search-card:hover .yr-thumb, .yr-history-row:hover .yr-thumb {{ background-color: #D8C9BC; }}
                 .yr-rail-sep {{ background-color: alpha(#1C1512, 0.12); }}
                 .yr-chip {{ background-color: alpha(#1C1512, 0.08); }}
                 .yr-chip:checked, .yr-chip.active {{ background-color: #1C1512; color: #FAF6F2; }}
                 .yr-segmented {{ background-color: alpha(#1C1512, 0.08); }}
                 .yr-segmented .yr-chip:checked {{ background-color: #1C1512; color: #FAF6F2; }}
                 .yr-search, entry.yr-search, searchentry.yr-search {{ background-color: alpha(#1C1512, 0.07); }}
                 .yr-btn {{ background-color: alpha(#1C1512, 0.09); }}
                 .yr-pref-card {{ background-color: alpha(#1C1512, 0.05); }}
                 .yr-pref-sep {{ background-color: alpha(#1C1512, 0.08); }}"
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
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::Home));
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

    fn list_card_layout(&self) -> ListCardLayout {
        if self.is_narrow.get() {
            ListCardLayout::Stacked
        } else if self.is_compact.get() {
            ListCardLayout::Compact
        } else {
            ListCardLayout::Wide
        }
    }

    fn setup_breakpoint(self: &Rc<Self>, bottom_bar: &GtkBox) {
        let tiers: [(&str, bool, bool, u32, u32); 4] = [
            ("max-width: 600px", true, true, 1, 2),
            ("min-width: 601px and max-width: 820px", false, true, 2, 3),
            ("min-width: 821px and max-width: 1120px", false, false, 3, 4),
            ("min-width: 1121px", false, false, 4, 6),
        ];

        for (cond_str, narrow, compact, g_cols, s_cols) in tiers {
            if let Ok(cond) = adw::BreakpointCondition::parse(cond_str) {
                let bp = adw::Breakpoint::new(cond);
                let weak = Rc::downgrade(self);
                let b_bar = bottom_bar.clone();
                bp.connect_apply(move |_| {
                    if let Some(w) = weak.upgrade() {
                        let prev_layout = w.list_card_layout();
                        w.is_narrow.set(narrow);
                        w.is_compact.set(compact);
                        w.grid_cols.set(g_cols);
                        w.shorts_cols.set(s_cols);
                        w.rail.container.set_visible(!narrow);
                        b_bar.set_visible(narrow);
                        w.player_card.set_narrow_mode(narrow);
                        w.apply_side_margins();
                        w.update_flow_columns();
                        w.notify_channels_layout(narrow, compact, g_cols, s_cols);

                        let next_layout = w.list_card_layout();
                        if prev_layout != next_layout {
                            w.refresh_responsive_list_cards();
                        }
                    }
                });
                self.window.add_breakpoint(bp);
            }
        }
    }

    fn apply_side_margins(&self) {
        let side = if self.is_narrow.get() {
            16
        } else if self.is_compact.get() {
            20
        } else {
            28
        };
        for bx in [
            &self.chips_box,
            &self.subs_strip_box,
            &self.history_header_box,
            &self.section_header_row,
        ] {
            bx.set_margin_start(side);
            bx.set_margin_end(side);
        }
        self.flow_box.set_margin_start(side);
        self.flow_box.set_margin_end(side);
        self.prefs_clamp.set_margin_start(side);
        self.prefs_clamp.set_margin_end(side);
    }

    fn refresh_responsive_list_cards(self: &Rc<Self>) {
        let av = self.active_view.borrow().clone();
        match av {
            ActiveView::Search(_) => {
                if self.flow_box.is_visible() && !self.current_videos.borrow().is_empty() {
                    let vids = self.current_videos.borrow().clone();
                    let keep_more = self.load_more_btn.is_visible();
                    self.populate_videos(vids, ViewMode::SearchList);
                    self.load_more_btn.set_visible(keep_more);
                }
            }
            ActiveView::History => {
                self.load_history();
            }
            _ => {}
        }
    }

    fn notify_channels_layout(&self, narrow: bool, compact: bool, grid_cols: u32, shorts_cols: u32) {
        for cp in self.open_channels.borrow().iter() {
            cp.set_layout(narrow, compact, grid_cols, shorts_cols);
        }
    }

    fn set_active_nav(&self, tab: Option<RailTab>) {
        self.rail.set_active(tab);
        self.bottom_home.remove_css_class("active");
        self.bottom_subs.remove_css_class("active");
        self.bottom_hist.remove_css_class("active");
        self.bottom_settings.remove_css_class("active");

        if let Some(t) = tab {
            match t {
                RailTab::Home => self.bottom_home.add_css_class("active"),
                RailTab::Subscriptions => self.bottom_subs.add_css_class("active"),
                RailTab::History => self.bottom_hist.add_css_class("active"),
                RailTab::Settings => self.bottom_settings.add_css_class("active"),
            }
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

    fn clear_search_bar_quietly(&self) {
        if !self.search_entry.text().is_empty() {
            self.suppress_search_clear.set(true);
            self.search_entry.set_text("");
            self.suppress_search_clear.set(false);
        }
    }

    fn update_back_button_visibility(&self) {
        let on_subpage = self
            .nav_view
            .visible_page()
            .and_then(|p| p.tag())
            .map(|t| t.as_str() != "home")
            .unwrap_or(false);
        let on_search = matches!(*self.active_view.borrow(), ActiveView::Search(_));
        self.back_btn.set_visible(on_subpage || on_search);
    }

    fn go_back(self: &Rc<Self>) {
        let on_subpage = self
            .nav_view
            .visible_page()
            .and_then(|p| p.tag())
            .map(|t| t.as_str() != "home")
            .unwrap_or(false);
        if on_subpage {
            self.nav_view.pop();
            return;
        }
        if matches!(*self.active_view.borrow(), ActiveView::Search(_)) {
            self.clear_search_bar_quietly();
            self.restore_previous_non_search_view();
        }
    }

    fn restore_previous_non_search_view(self: &Rc<Self>) {
        let prev = self.prev_non_search_view.borrow().clone();
        match prev {
            ActiveView::HomeCategory(cat) => {
                self.set_active_nav(Some(RailTab::Home));
                match cat.as_str() {
                    "Gaming" => self.highlight_chip(Some(&self.chip_gaming)),
                    "Music" => self.highlight_chip(Some(&self.chip_music)),
                    "News" => self.highlight_chip(Some(&self.chip_news)),
                    _ => self.highlight_chip(Some(&self.chip_trending)),
                }
                self.search_category(&cat);
            }
            ActiveView::WatchLater => {
                self.set_active_nav(Some(RailTab::Home));
                self.highlight_chip(Some(&self.chip_watch_later));
                self.load_watch_later();
            }
            ActiveView::Subscriptions => {
                self.set_active_nav(Some(RailTab::Subscriptions));
                self.load_subscriptions();
            }
            ActiveView::History => {
                self.set_active_nav(Some(RailTab::History));
                self.load_history();
            }
            ActiveView::Preferences => {
                self.open_settings();
            }
            _ => {
                self.set_active_nav(Some(RailTab::Home));
                self.highlight_chip(Some(&self.chip_trending));
                self.load_trending();
            }
        }
    }

    fn update_flow_columns(&self) {
        let cols = self.grid_cols.get();
        match self.view_mode.get() {
            ViewMode::Grid => {
                self.flow_box.set_homogeneous(true);
                self.flow_box.set_min_children_per_line(cols);
                self.flow_box.set_max_children_per_line(cols);
                self.flow_box.set_column_spacing(20);
                self.flow_box.set_row_spacing(24);
            }
            ViewMode::SearchList => {
                self.flow_box.set_homogeneous(false);
                self.flow_box.set_min_children_per_line(1);
                self.flow_box.set_max_children_per_line(1);
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

        // Window keyboard shortcuts: '/', 'Ctrl+F', 'Alt+Left', 'Esc'
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
                    w.go_back();
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
                let in_editable = gtk4::prelude::GtkWindowExt::focus(&w.window)
                    .map(|f| f.is::<Editable>())
                    .unwrap_or(false);
                if !in_editable {
                    w.search_entry.grab_focus();
                    return glib::Propagation::Stop;
                }
            }

            if key == gdk::Key::Escape {
                let search_focused = gtk4::prelude::GtkWindowExt::focus(&w.window)
                    .map(|f| {
                        f.is_ancestor(&w.search_entry)
                            || &f == w.search_entry.upcast_ref::<gtk4::Widget>()
                    })
                    .unwrap_or(false);
                if search_focused {
                    gtk4::prelude::GtkWindowExt::set_focus(&w.window, gtk4::Widget::NONE);
                    return glib::Propagation::Stop;
                }
            }

            glib::Propagation::Proceed
        });
        self.window.add_controller(key_ctrl);

        // Header Back Button + NavigationView page tracking
        let weak = Rc::downgrade(self);
        self.back_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.go_back();
            }
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
            w.update_back_button_visibility();

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

        // Load more button
        let weak = Rc::downgrade(self);
        self.load_more_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.load_more_current_feed();
            }
        });

        // Search activation + clearing search returns to previous view
        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |entry| {
            let query = entry.text().trim().to_string();
            if let Some(w) = weak.upgrade() {
                if !query.is_empty() {
                    gtk4::prelude::GtkWindowExt::set_focus(&w.window, gtk4::Widget::NONE);
                    w.search(&query);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |entry| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.suppress_search_clear.get() {
                return;
            }
            if entry.text().is_empty() && matches!(*w.active_view.borrow(), ActiveView::Search(_)) {
                w.restore_previous_non_search_view();
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
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::Home));
                w.highlight_chip(Some(&w.chip_trending));
                w.load_trending();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.home_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::Home));
                w.highlight_chip(Some(&w.chip_trending));
                w.load_trending();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.subs_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::Subscriptions));
                w.load_subscriptions();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.hist_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::History));
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
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::Home));
                w.highlight_chip(Some(&w.chip_trending));
                w.load_trending();
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_subs.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::Subscriptions));
                w.load_subscriptions();
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_hist.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::History));
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
                    w.player_card.set_loading(true);
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
                w.player_card.set_loading(false);
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
                w.player_card.set_loading(false);
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
                w.player_card.set_loading(false);
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.stop();
                    session.clear();
                }
                w.player_card.set_queue(vec![], 0, false);
                w.player_card.root.set_visible(false);
            }
        });
    }

    fn sync_preferences_ui(&self, settings: &AppSettings) {
        self.pref_scheme_btn.set_label(match settings.color_scheme.as_str() {
            "system" => "System",
            "light" => "Light",
            _ => "Dark",
        });
        self.pref_accent_btn.set_label(match settings.accent_color.as_str() {
            "forest" => "Forest",
            "plum" => "Plum",
            "teal" => "Teal",
            _ => "Terracotta",
        });
        self.pref_quality_btn
            .set_label(Quality::from_setting(&settings.default_quality).label());
        self.pref_hwdec_switch.set_active(settings.hardware_decoding);
        if self.pref_player_entry.text().as_str() != settings.external_player {
            self.pref_player_entry.set_text(&settings.external_player);
        }
        self.pref_hist_switch.set_active(settings.save_history);
    }

    fn wire_preferences_signals(
        self: &Rc<Self>,
        clear_cache_btn: &Button,
        clear_hist_btn: &Button,
    ) {
        // Color scheme popover
        attach_choice_popover(
            &self.pref_scheme_btn,
            &[("dark", "Dark"), ("system", "Follow system"), ("light", "Light")],
            {
                let weak = Rc::downgrade(self);
                move |key| {
                    if let Some(w) = weak.upgrade() {
                        let mut s = w.settings_store.load();
                        s.color_scheme = key.to_string();
                        w.settings_store.save(&s);
                        w.apply_appearance(&s);
                        w.sync_preferences_ui(&s);
                    }
                }
            },
        );

        // Accent color popover
        attach_choice_popover(
            &self.pref_accent_btn,
            &[
                ("terracotta", "Terracotta"),
                ("forest", "Forest"),
                ("plum", "Plum"),
                ("teal", "Teal"),
            ],
            {
                let weak = Rc::downgrade(self);
                move |key| {
                    if let Some(w) = weak.upgrade() {
                        let mut s = w.settings_store.load();
                        s.accent_color = key.to_string();
                        w.settings_store.save(&s);
                        w.apply_appearance(&s);
                        w.sync_preferences_ui(&s);
                    }
                }
            },
        );

        // Default quality popover
        attach_choice_popover(
            &self.pref_quality_btn,
            &[
                (Quality::P360.as_setting(), "360p"),
                (Quality::P720.as_setting(), "720p"),
                (Quality::P1080.as_setting(), "1080p"),
                (Quality::AudioOnly.as_setting(), "Audio only"),
            ],
            {
                let weak = Rc::downgrade(self);
                move |key| {
                    if let Some(w) = weak.upgrade() {
                        let mut s = w.settings_store.load();
                        s.default_quality = Quality::from_setting(key).as_setting().to_string();
                        w.settings_store.save(&s);
                        w.sync_preferences_ui(&s);
                    }
                }
            },
        );

        // Hardware decoding switch
        let weak = Rc::downgrade(self);
        self.pref_hwdec_switch.connect_state_set(move |_, active| {
            if let Some(w) = weak.upgrade() {
                let mut s = w.settings_store.load();
                s.hardware_decoding = active;
                w.settings_store.save(&s);
                if let Some(sess) = w.mpv_session.borrow().as_ref() {
                    sess.set_hardware_decoding(active);
                }
            }
            glib::Propagation::Proceed
        });

        // External player command
        let store_pl = self.settings_store.clone();
        self.pref_player_entry.connect_changed(move |entry| {
            let mut s = store_pl.load();
            let txt = entry.text().trim().to_string();
            s.external_player = if txt.is_empty() {
                "mpv".to_string()
            } else {
                txt
            };
            store_pl.save(&s);
        });

        // Save watch history switch
        let store_h = self.settings_store.clone();
        self.pref_hist_switch.connect_state_set(move |_, active| {
            let mut s = store_h.load();
            s.save_history = active;
            store_h.save(&s);
            glib::Propagation::Proceed
        });

        // Clear thumbnail cache
        let weak = Rc::downgrade(self);
        clear_cache_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.image_loader.clear_all();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Thumbnail cache cleared"));
            }
        });

        // Clear watch history
        let weak = Rc::downgrade(self);
        clear_hist_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.history_store.clear();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Watch history cleared"));
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
        set_pointer(&reset_btn);
        let apply_btn = Button::with_label("Apply");
        apply_btn.add_css_class("yr-chip");
        apply_btn.add_css_class("active");
        set_pointer(&apply_btn);

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

        self.player_card.set_queue(vec![summary], 0, false);
        self.player_card.set_playing(true);
        self.player_card.set_loading(true);
    }

    pub fn play_shorts_queue(self: &Rc<Self>, shorts: Vec<VideoSummary>, start_index: usize) {
        if shorts.is_empty() {
            return;
        }
        let clamped_start = start_index.min(shorts.len() - 1);
        let session = self.ensure_mpv_session();
        self.mpv_became_active.set(false);
        self.mpv_pos.set(0.0);
        self.mpv_duration
            .set(shorts[clamped_start].duration_seconds.unwrap_or(0) as f64);

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

        self.player_card.set_queue(shorts, clamped_start, true);
        self.player_card.set_playing(true);
        self.player_card.set_loading(true);
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
            "Downloading \"{short_title}\" to Downloads…"
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
                    MpvEvent::Loading => {
                        w.player_card.set_loading(true);
                    }
                    MpvEvent::Loaded => {
                        w.player_card.set_loading(false);
                    }
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
                            w.player_card.set_loading(false);
                            w.player_card.set_playing(false);
                        }
                    }
                    MpvEvent::Error(msg) => {
                        w.player_card.set_loading(false);
                        w.toast_overlay.add_toast(adw::Toast::new(&msg));
                    }
                    MpvEvent::ProcessExited => {
                        *w.mpv_session.borrow_mut() = None;
                        w.player_card.set_loading(false);
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
            self.is_compact.get(),
            self.grid_cols.get(),
            self.shorts_cols.get(),
        );

        self.open_channels.borrow_mut().push(channel_page.clone());
        self.nav_view.push(&channel_page.page);

        let (ch_tx, ch_rx) =
            async_channel::bounded::<Result<(ChannelDetails, PagedVideos), String>>(1);
        let (sh_tx, sh_rx) = async_channel::bounded::<Result<PagedVideos, String>>(1);

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
                        Ok((details, paged)) => {
                            cp.set_channel_details(details);
                            cp.set_videos(paged);
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
            if let Ok(Ok(paged)) = sh_rx.recv().await {
                if let Some(cp) = cp_weak.upgrade() {
                    cp.set_shorts(paged);
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
        if !matches!(view, ActiveView::Search(_)) {
            *self.prev_non_search_view.borrow_mut() = view.clone();
        }
        *self.active_view.borrow_mut() = view.clone();
        *self.feed_cursor.borrow_mut() = None;
        self.is_loading_more.set(false);
        self.load_more_btn.set_visible(false);
        self.load_more_btn.set_sensitive(true);
        self.load_more_btn.set_label("Load more");
        self.view_mode.set(mode);

        let is_prefs = view == ActiveView::Preferences;
        self.feed_box.set_visible(!is_prefs);
        self.prefs_clamp.set_visible(is_prefs);

        // Scroll back to top on view switches
        self.content_scrolled.vadjustment().set_value(0.0);

        if is_prefs {
            self.update_back_button_visibility();
            return;
        }

        self.chips_scroll.set_visible(show_chips);
        self.subs_strip_box.set_visible(show_subs);
        self.history_header_box.set_visible(show_history_header);

        let is_search = matches!(view, ActiveView::Search(_));
        let is_wl = view == ActiveView::WatchLater;
        self.search_filters_btn.set_visible(is_search);
        self.clear_watch_later_btn.set_visible(is_wl);

        if is_search {
            self.section_title.remove_css_class("yr-h2");
            self.section_title.add_css_class("yr-search-heading");
        } else {
            self.section_title.remove_css_class("yr-search-heading");
            self.section_title.add_css_class("yr-h2");
        }

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
        self.update_back_button_visibility();
    }

    pub fn search(self: &Rc<Self>, query: &str) {
        let req_id = self.next_request_id();
        self.set_active_nav(None);
        self.prepare_view_shell(
            ActiveView::Search(query.to_string()),
            ViewMode::SearchList,
            false,
            false,
            false,
            Some(&format!("Results for “{query}”")),
        );
        self.show_skeletons_search(6);

        let (sender, receiver) = async_channel::bounded::<Result<PagedVideos, String>>(1);
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
                Ok(paged) => {
                    if paged.items.is_empty() {
                        let weak_clear = Rc::downgrade(&w);
                        w.show_status_state(
                            "system-search-symbolic",
                            "No results",
                            &format!(
                                "Nothing matched “{q_ui}”. Check the spelling or try different words."
                            ),
                            Some(("Clear search", Rc::new(move || {
                                if let Some(win) = weak_clear.upgrade() {
                                    win.clear_search_bar_quietly();
                                    win.restore_previous_non_search_view();
                                }
                            }))),
                        );
                    } else {
                        let has_more = paged.next.is_some();
                        *w.feed_cursor.borrow_mut() = paged.next;
                        w.populate_videos(paged.items, ViewMode::SearchList);
                        w.load_more_btn.set_visible(has_more);
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

        let (sender, receiver) = async_channel::bounded::<Result<PagedVideos, String>>(1);
        let engine = self.engine.clone();
        let cat = category.to_string();
        let cat_ui = category.to_string();

        self.runtime.spawn(async move {
            let res = engine.category_feed(&cat).await;
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
                Ok(paged) => {
                    if paged.items.is_empty() {
                        w.show_status_state(
                            "system-search-symbolic",
                            "No videos found",
                            "Please try again in a moment.",
                            None,
                        );
                    } else {
                        let has_more = paged.next.is_some();
                        *w.feed_cursor.borrow_mut() = paged.next;
                        w.populate_videos(paged.items, ViewMode::Grid);
                        w.load_more_btn.set_visible(has_more);
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

    fn load_more_current_feed(self: &Rc<Self>) {
        if self.is_loading_more.get() {
            return;
        }
        let Some(cursor) = self.feed_cursor.borrow().clone() else {
            self.load_more_btn.set_visible(false);
            return;
        };
        let req_id = self.request_seq.get();
        let mode = self.view_mode.get();
        let filter_shorts = matches!(*self.active_view.borrow(), ActiveView::HomeCategory(_));

        self.is_loading_more.set(true);
        self.load_more_btn.set_sensitive(false);
        self.load_more_btn.set_label("Loading more…");

        let engine = self.engine.clone();
        let (tx, rx) = async_channel::bounded::<Result<PagedVideos, String>>(1);
        self.runtime.spawn(async move {
            let res = engine.load_more(&cursor, None, false, filter_shorts).await;
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
            if w.request_seq.get() != req_id {
                return;
            }
            w.is_loading_more.set(false);
            w.load_more_btn.set_sensitive(true);
            w.load_more_btn.set_label("Load more");

            match res {
                Ok(paged) => {
                    let has_more = paged.next.is_some();
                    *w.feed_cursor.borrow_mut() = paged.next;
                    w.append_videos(paged.items, mode);
                    w.load_more_btn.set_visible(has_more);
                }
                Err(err) => {
                    w.toast_overlay.add_toast(adw::Toast::new(&err));
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
                "Click “Watch later” on any video to save it here for later.",
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
                "video-display-symbolic",
                "No subscriptions yet",
                "Open any channel and click Subscribe to see their latest uploads here.",
                Some(("Browse Trending", Rc::new(move || {
                    if let Some(win) = weak_home.upgrade() {
                        win.set_active_nav(Some(RailTab::Home));
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
            set_pointer(&item_box);

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
                            "video-display-symbolic",
                            "No recent uploads",
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
                "No watch history",
                "Videos you play in MPV will appear here.",
                None,
            );
            return;
        }

        let actions = self.make_card_actions();
        let layout = self.list_card_layout();

        for entry in entries {
            let weak_rm = Rc::downgrade(self);
            let vid = entry.video.id.clone();
            let card = VideoCard::new_history(&entry, layout, move || {
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

            let sum_mid = entry.video.clone();
            let on_play_mid = actions.on_play.clone();
            let def_q_mid = actions.default_quality.clone();
            card.connect_middle_click(move || {
                on_play_mid(sum_mid.clone(), def_q_mid());
            });

            ContextMenu::attach(&card.container, &entry.video, &actions);
            self.flow_box.append(&card.container);
        }
    }

    pub fn open_settings(self: &Rc<Self>) {
        let _req_id = self.next_request_id();
        self.clear_search_bar_quietly();
        self.set_active_nav(Some(RailTab::Settings));
        let current = self.settings_store.load();
        self.sync_preferences_ui(&current);
        self.prepare_view_shell(
            ActiveView::Preferences,
            ViewMode::Grid,
            false,
            false,
            false,
            None,
        );
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
            ("Enter", "Run search / Play in Details"),
            ("Middle-click", "Play video directly in MPV"),
            ("Alt + Left", "Go back"),
            ("Space", "Pause or resume Player Card"),
        ];

        for (keys, desc) in items {
            let row = GtkBox::new(Orientation::Horizontal, 12);
            let desc_lbl = Label::new(Some(desc));
            desc_lbl.add_css_class("yr-meta");
            desc_lbl.set_hexpand(true);
            desc_lbl.set_halign(Align::Start);
            desc_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

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
        set_pointer(&close_btn);
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

        let logo = GtkBox::new(Orientation::Vertical, 0);
        logo.add_css_class("yr-logo");
        logo.set_halign(Align::Center);
        logo.append(&make_icon(KalamIcon::LogoPlay, 38));
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
        desc.set_max_width_chars(38);
        vbox.append(&desc);

        let close_btn = Button::with_label("Close");
        close_btn.add_css_class("yr-btn");
        close_btn.add_css_class("primary");
        close_btn.set_halign(Align::Center);
        close_btn.set_margin_top(8);
        set_pointer(&close_btn);
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
        self.load_more_btn.set_visible(false);
        self.flow_box.set_visible(true);
        self.current_videos.borrow_mut().clear();
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
        self.load_more_btn.set_visible(false);
        self.flow_box.set_visible(true);
        self.current_videos.borrow_mut().clear();
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.view_mode.set(ViewMode::SearchList);
        self.update_flow_columns();
        let layout = self.list_card_layout();
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_search_skeleton(layout));
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
        self.current_videos.borrow_mut().clear();
        self.flow_box.set_visible(false);
        self.load_more_btn.set_visible(false);

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
        *self.current_videos.borrow_mut() = Vec::new();
        self.append_videos(videos, mode);
    }

    fn append_videos(self: &Rc<Self>, videos: Vec<VideoSummary>, mode: ViewMode) {
        let actions = self.make_card_actions();
        let layout = self.list_card_layout();

        for summary in videos {
            let card = match mode {
                ViewMode::Grid => VideoCard::new_grid(&summary),
                ViewMode::SearchList => VideoCard::new_search(&summary, layout),
            };

            let weak_click = Rc::downgrade(self);
            let sum_cl = summary.clone();
            let act_cl = actions.clone();
            card.connect_primary_click(move || {
                if let Some(w) = weak_click.upgrade() {
                    DetailsSheet::show(&w.window, &sum_cl, &act_cl);
                }
            });

            let sum_mid = summary.clone();
            let on_play_mid = actions.on_play.clone();
            let def_q_mid = actions.default_quality.clone();
            card.connect_middle_click(move || {
                on_play_mid(sum_mid.clone(), def_q_mid());
            });

            if !summary.channel_id.is_empty() {
                let weak_chan = Rc::downgrade(self);
                let cid = summary.channel_id.clone();
                let cname = summary.channel_name.clone();
                card.connect_channel_click(move || {
                    if let Some(w) = weak_chan.upgrade() {
                        w.open_channel(&cid, &cname);
                    }
                });
            }

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
            self.current_videos.borrow_mut().push(summary);
        }
    }

    pub fn present(&self) {
        self.window.present();
    }
}

#[allow(clippy::type_complexity)]
fn build_inline_preferences_view() -> (
    adw::Clamp,
    Button,
    Button,
    Button,
    Switch,
    Entry,
    Switch,
    Button,
    Button,
) {
    let col = GtkBox::new(Orientation::Vertical, 22);
    col.set_margin_top(12);
    col.set_margin_bottom(40);
    col.set_hexpand(true);

    let title = Label::new(Some("Preferences"));
    title.add_css_class("yr-h1");
    title.set_halign(Align::Start);
    col.append(&title);

    // --- Group 1: Appearance ---
    let g_app = GtkBox::new(Orientation::Vertical, 8);
    let g_app_lbl = Label::new(Some("Appearance"));
    g_app_lbl.add_css_class("yr-pref-section");
    g_app_lbl.set_halign(Align::Start);
    g_app.append(&g_app_lbl);

    let card_app = GtkBox::new(Orientation::Vertical, 0);
    card_app.add_css_class("yr-pref-card");

    let scheme_btn = Button::with_label("Dark");
    scheme_btn.add_css_class("yr-btn");
    scheme_btn.set_valign(Align::Center);
    set_pointer(&scheme_btn);
    card_app.append(&make_pref_row(
        "Color scheme",
        "Follow the system or force a mode",
        &scheme_btn,
    ));
    card_app.append(&make_pref_sep());

    let accent_btn = Button::with_label("Terracotta");
    accent_btn.add_css_class("yr-btn");
    accent_btn.set_valign(Align::Center);
    set_pointer(&accent_btn);
    card_app.append(&make_pref_row(
        "Accent color",
        "Terracotta, Forest, Plum or Teal",
        &accent_btn,
    ));

    g_app.append(&card_app);
    col.append(&g_app);

    // --- Group 2: Playback ---
    let g_play = GtkBox::new(Orientation::Vertical, 8);
    let g_play_lbl = Label::new(Some("Playback"));
    g_play_lbl.add_css_class("yr-pref-section");
    g_play_lbl.set_halign(Align::Start);
    g_play.append(&g_play_lbl);

    let card_play = GtkBox::new(Orientation::Vertical, 0);
    card_play.add_css_class("yr-pref-card");

    let quality_btn = Button::with_label("360p");
    quality_btn.add_css_class("yr-btn");
    quality_btn.set_valign(Align::Center);
    set_pointer(&quality_btn);
    card_play.append(&make_pref_row(
        "Default quality",
        "Quality the video starts with",
        &quality_btn,
    ));
    card_play.append(&make_pref_sep());

    let hwdec_switch = Switch::new();
    hwdec_switch.set_valign(Align::Center);
    set_pointer(&hwdec_switch);
    card_play.append(&make_pref_row(
        "Hardware decoding",
        "Use VA-API when available",
        &hwdec_switch,
    ));
    card_play.append(&make_pref_sep());

    let player_entry = Entry::builder()
        .text("mpv")
        .width_chars(10)
        .max_width_chars(16)
        .valign(Align::Center)
        .build();
    player_entry.add_css_class("yr-search");
    card_play.append(&make_pref_row(
        "Player command",
        "Program that plays videos",
        &player_entry,
    ));

    g_play.append(&card_play);
    col.append(&g_play);

    // --- Group 3: Data ---
    let g_data = GtkBox::new(Orientation::Vertical, 8);
    let g_data_lbl = Label::new(Some("Data"));
    g_data_lbl.add_css_class("yr-pref-section");
    g_data_lbl.set_halign(Align::Start);
    g_data.append(&g_data_lbl);

    let card_data = GtkBox::new(Orientation::Vertical, 0);
    card_data.add_css_class("yr-pref-card");

    let clear_cache_btn = Button::with_label("Clear");
    clear_cache_btn.add_css_class("yr-btn");
    clear_cache_btn.set_valign(Align::Center);
    set_pointer(&clear_cache_btn);
    card_data.append(&make_pref_row(
        "Clear thumbnail cache",
        "Free disk space used by images",
        &clear_cache_btn,
    ));
    card_data.append(&make_pref_sep());

    let hist_switch = Switch::new();
    hist_switch.set_valign(Align::Center);
    set_pointer(&hist_switch);
    card_data.append(&make_pref_row(
        "Save watch history",
        "Stored locally on this device",
        &hist_switch,
    ));
    card_data.append(&make_pref_sep());

    let clear_hist_btn = Button::with_label("Clear");
    clear_hist_btn.add_css_class("yr-btn");
    clear_hist_btn.set_valign(Align::Center);
    set_pointer(&clear_hist_btn);
    card_data.append(&make_pref_row(
        "Clear watch history",
        "Remove all saved history entries",
        &clear_hist_btn,
    ));

    g_data.append(&card_data);
    col.append(&g_data);

    let clamp = adw::Clamp::builder()
        .maximum_size(560)
        .tightening_threshold(400)
        .hexpand(true)
        .margin_start(28)
        .margin_end(28)
        .visible(false)
        .child(&col)
        .build();

    (
        clamp,
        scheme_btn,
        accent_btn,
        quality_btn,
        hwdec_switch,
        player_entry,
        hist_switch,
        clear_cache_btn,
        clear_hist_btn,
    )
}

fn make_pref_row(title: &str, subtitle: &str, suffix: &impl IsA<gtk4::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 16);
    row.add_css_class("yr-pref-row");
    row.set_valign(Align::Center);

    let text_col = GtkBox::new(Orientation::Vertical, 2);
    text_col.set_hexpand(true);
    text_col.set_valign(Align::Center);

    let t = Label::new(Some(title));
    t.add_css_class("yr-title");
    t.set_halign(Align::Start);
    t.set_xalign(0.0);
    t.set_ellipsize(gtk4::pango::EllipsizeMode::End);

    let sub = Label::new(Some(subtitle));
    sub.add_css_class("yr-meta");
    sub.set_halign(Align::Start);
    sub.set_xalign(0.0);
    sub.set_wrap(true);

    text_col.append(&t);
    text_col.append(&sub);

    row.append(&text_col);
    row.append(suffix);
    row
}

fn make_pref_sep() -> GtkBox {
    let sep = GtkBox::new(Orientation::Horizontal, 0);
    sep.add_css_class("yr-pref-sep");
    sep.set_hexpand(true);
    sep
}

fn attach_choice_popover(
    button: &Button,
    options: &[(&'static str, &'static str)],
    on_select: impl Fn(&'static str) + 'static,
) {
    let popover = Popover::new();
    popover.set_parent(button);
    popover.set_has_arrow(false);

    let pop_cleanup = popover.clone();
    button.connect_destroy(move |_| {
        pop_cleanup.unparent();
    });

    let vbox = GtkBox::new(Orientation::Vertical, 2);
    vbox.set_width_request(180);

    let cb = Rc::new(on_select);
    for &(key, label) in options {
        let item = make_filter_item(label, false);
        let pop_weak = popover.downgrade();
        let cb_cl = cb.clone();
        item.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            cb_cl(key);
        });
        vbox.append(&item);
    }

    popover.set_child(Some(&vbox));
    let pop_weak = popover.downgrade();
    button.connect_clicked(move |_| {
        if let Some(pop) = pop_weak.upgrade() {
            pop.popup();
        }
    });
}

fn create_chip(label: &str, is_active: bool) -> Button {
    let btn = Button::builder().label(label).build();
    btn.add_css_class("yr-chip");
    set_pointer(&btn);
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

fn create_bottom_nav_btn(icon: KalamIcon, tooltip: &str, is_active: bool) -> Button {
    let btn = Button::new();
    btn.set_child(Some(&make_icon(icon, 22)));
    btn.add_css_class("yr-rail-btn");
    btn.set_tooltip_text(Some(tooltip));
    btn.set_hexpand(true);
    btn.set_halign(Align::Center);
    btn.set_width_request(44);
    btn.set_height_request(44);
    set_pointer(&btn);
    if is_active {
        btn.add_css_class("active");
    }
    btn
}

fn make_filter_item(label: &str, selected: bool) -> Button {
    let btn = Button::builder().label(label).halign(Align::Fill).build();
    btn.add_css_class("flat");
    btn.add_css_class("yr-menu-item");
    set_pointer(&btn);
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
