use crate::backend::cache::Cache;
use crate::backend::engine::{
    Engine, PageCursor, PagedSearchItems, PagedTrending, PagedVideos, RemotePlaylist, ResolvedUrl,
};
use crate::backend::history::HistoryStore;
use crate::backend::models::{
    format_queue_total_duration, looks_like_youtube_url, ChannelDetails, DurationFilter,
    PlaylistSearchSummary, SearchItem, SearchSortOrder, SearchTab, UploadDateFilter,
    VideoCollaborator, VideoSummary,
};
use crate::backend::playlists::PlaylistStore;
use crate::backend::search_history::SearchHistoryStore;
use crate::backend::settings::{AppSettings, SettingsStore};
use crate::backend::subscriptions::{SubscribedChannel, SubscriptionStore};
use crate::backend::watch_later::WatchLaterStore;
use crate::player::mpv::Quality;
use crate::player::mpv_ipc::{MpvEvent, MpvQueueItem, MpvSession};
use crate::ui::channel_page::{ChannelPage, ShortsPlayHandler};
use crate::ui::details_sheet::{CardActions, DetailsSheet};
use crate::ui::icons::{make_icon, set_pointer, KalamIcon};
use crate::ui::image_loader::ImageLoader;
use crate::ui::player_card::PlayerCard;
use crate::ui::rail::{Rail, RailTab};
use crate::ui::video_card::{
    ChannelResultCard, ListCardLayout, PlaylistResultCard, SavedPlaylistCard, VideoCard,
};
use adw::prelude::*;
use gtk4::gdk::{self, Display};
use gtk4::{
    gio, glib, Align, Box as GtkBox, Button, CssProvider, Editable, Entry, EventControllerFocus,
    EventControllerKey, FlowBox, GestureClick, Image, Label, MenuButton,
    Orientation, Overlay, Popover, PropagationPhase, ScrolledWindow, SelectionMode, Separator,
    Switch,
};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

static PLAYLIST_SHUFFLE_SEED: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    ShortsGrid,
    SearchList,
}

#[derive(Clone)]
pub struct QueueContinuation {
    pub cursor: PageCursor,
    pub fallback_details: Option<ChannelDetails>,
    pub is_shorts: bool,
}

#[derive(Clone)]
enum NavHistoryEntry {
    MainView {
        view: ActiveView,
        search_tab: SearchTab,
        search_date: UploadDateFilter,
        search_dur: DurationFilter,
        search_sort: SearchSortOrder,
    },
    Channel {
        id: String,
        name: String,
    },
}

#[derive(Clone, PartialEq, Eq)]
enum ActiveView {
    HomeTrending,
    HomeCategory(String),
    WatchLater,
    Search(String),
    Subscriptions,
    Playlists,
    SavedPlaylistDetail(String),
    RemotePlaylist(String),
    History,
    Preferences,
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    back_btn: Button,
    refresh_btn: Button,
    pub search_entry: Entry,
    search_clear_btn: Button,
    search_dropdown_clamp: adw::Clamp,
    search_suggest_box: GtkBox,
    suggest_items: RefCell<Vec<(String, GtkBox)>>,
    suggest_selected_idx: Cell<isize>,
    suggest_seq: Cell<u64>,
    typed_before_nav: RefCell<String>,
    suppress_search_clear: Cell<bool>,
    pub rail: Rail,
    bottom_home: Button,
    bottom_subs: Button,
    bottom_playlists: Button,
    bottom_watch_later: Button,
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
    new_playlist_btn: Button,
    search_tabs_scroll: ScrolledWindow,
    search_tabs_box: GtkBox,
    stab_all: Button,
    stab_videos: Button,
    stab_shorts: Button,
    stab_channels: Button,
    stab_playlists: Button,
    playlist_header_box: GtkBox,
    playlist_title_lbl: Label,
    playlist_meta_lbl: Label,
    playlist_play_all_btn: Button,
    playlist_shuffle_btn: Button,
    playlist_save_remote_btn: Button,
    playlist_rename_btn: Button,
    playlist_delete_btn: Button,
    chips_scroll: ScrolledWindow,
    chips_box: GtkBox,
    chip_trending: Button,
    chip_gaming: Button,
    chip_music: Button,
    chip_news: Button,
    add_category_btn: Button,
    custom_category_buttons: RefCell<Vec<(String, Button)>>,
    pub subs_strip_box: GtkBox,
    pub subs_avatars_row: GtkBox,
    pub history_header_box: GtkBox,
    pub toast_overlay: adw::ToastOverlay,
    pub player_card: Rc<PlayerCard>,
    pub mpv_session: RefCell<Option<Arc<MpvSession>>>,
    mpv_player_cmd: RefCell<String>,
    mpv_became_active: Cell<bool>,
    playback_seq: Cell<u64>,
    mpv_sync_gen: Cell<u64>,
    mpv_acked_gen: Cell<u64>,
    queue_continuation: RefCell<Option<QueueContinuation>>,
    queue_cont_fetching: Cell<bool>,
    playback_quality: Cell<Quality>,
    pub mpv_pos: Cell<f64>,
    pub mpv_duration: Cell<f64>,
    is_narrow: Cell<bool>,
    is_compact: Cell<bool>,
    grid_cols: Cell<u32>,
    shorts_cols: Cell<u32>,
    view_mode: Cell<ViewMode>,
    active_view: RefCell<ActiveView>,
    nav_history: RefCell<Vec<NavHistoryEntry>>,
    suppress_nav_push: Cell<bool>,
    channel_over_transient_home: Cell<bool>,
    current_videos: RefCell<Vec<VideoSummary>>,
    current_search_items: RefCell<Vec<SearchItem>>,
    remote_playlist_data: RefCell<Option<RemotePlaylist>>,
    feed_cursor: RefCell<Option<PageCursor>>,
    trending_cursors: RefCell<Vec<PageCursor>>,
    is_loading_more: Cell<bool>,
    request_seq: Cell<u64>,
    search_tab: Cell<SearchTab>,
    search_date_filter: Cell<UploadDateFilter>,
    search_dur_filter: Cell<DurationFilter>,
    search_sort_order: Cell<SearchSortOrder>,
    selected_box: RefCell<Option<GtkBox>>,
    selected_action_cb: RefCell<Option<Rc<dyn Fn()>>>,
    open_channels: RefCell<Vec<Rc<ChannelPage>>>,
    home_pictures: RefCell<Vec<(String, String, bool, glib::WeakRef<gtk4::Picture>)>>,
    home_avatars: RefCell<Vec<(String, String, glib::WeakRef<adw::Avatar>)>>,
    home_textures_suspended: Cell<bool>,
    last_home_cull_y: Cell<f64>,
    theme_provider: CssProvider,
    pub image_loader: Rc<ImageLoader>,
    pub history_store: Arc<HistoryStore>,
    pub search_history_store: Arc<SearchHistoryStore>,
    pub playlist_store: Arc<PlaylistStore>,
    pub settings_store: Arc<SettingsStore>,
    pub subscription_store: Arc<SubscriptionStore>,
    pub watch_later_store: Arc<WatchLaterStore>,
    engine: Arc<Engine>,
    runtime: Arc<Runtime>,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let theme_provider = load_custom_css();

        // HeaderBar: borderless, centered search bar
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

        // Centered Search Bar (synchronous Entry + in-window Overlay dropdown for flicker-free Wayland UX)
        let search_clamp = adw::Clamp::builder()
            .maximum_size(440)
            .tightening_threshold(240)
            .hexpand(true)
            .build();

        let search_entry = Entry::builder()
            .placeholder_text("Search YouTube or paste link")
            .primary_icon_name("system-search-symbolic")
            .primary_icon_activatable(false)
            .primary_icon_sensitive(false)
            .width_request(120)
            .hexpand(true)
            .build();
        search_entry.add_css_class("yr-search");
        search_entry.add_css_class("yr-main-search");
        let search_clear_btn = Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Clear search")
            .visible(false)
            .build();
        search_clear_btn.add_css_class("flat");
        search_clear_btn.add_css_class("yr-search-clear");
        search_clear_btn.set_halign(Align::End);
        search_clear_btn.set_valign(Align::Center);
        search_clear_btn.set_margin_end(8);
        search_clear_btn.set_focus_on_click(false);
        set_pointer(&search_clear_btn);
        let search_overlay = Overlay::new();
        search_overlay.set_child(Some(&search_entry));
        search_overlay.add_overlay(&search_clear_btn);
        search_clamp.set_child(Some(&search_overlay));
        header_bar.set_title_widget(Some(&search_clamp));

        // Primary Menu + Refresh Buttons at the top right
        let menu_btn = MenuButton::new();
        menu_btn.set_child(Some(&make_icon(KalamIcon::Menu, 22)));
        menu_btn.set_tooltip_text(Some("Main menu"));
        menu_btn.set_has_frame(false);
        menu_btn.set_size_request(44, 44);
        menu_btn.add_css_class("yr-rail-btn");
        menu_btn.add_css_class("yr-menu-round");
        set_pointer(&menu_btn);

        let refresh_btn = Button::new();
        refresh_btn.set_child(Some(&make_icon(KalamIcon::Refresh, 19)));
        refresh_btn.set_tooltip_text(Some("Refresh (Ctrl+R / F5)"));
        refresh_btn.add_css_class("yr-rail-btn");
        set_pointer(&refresh_btn);

        let menu = gio::Menu::new();
        menu.append(Some("Refresh"), Some("app.refresh"));
        menu.append(Some("Playlists"), Some("app.playlists"));
        menu.append(Some("Watch Later"), Some("app.watch_later"));
        menu.append(Some("Preferences"), Some("app.settings"));
        menu.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        menu.append(Some("About youtube-rusty"), Some("app.about"));
        menu_btn.set_menu_model(Some(&menu));
        header_bar.pack_end(&menu_btn);
        header_bar.pack_end(&refresh_btn);

        // Full-height Left Navigation Rail (68px)
        let rail = Rail::new();

        // Scrollable Home Chips Row (hidden scrollbars)
        let chips_box = GtkBox::new(Orientation::Horizontal, 8);
        chips_box.set_margin_start(28);
        chips_box.set_margin_end(28);
        chips_box.set_margin_top(10);
        chips_box.set_margin_bottom(12);

        let chip_trending = create_chip("Trending", true);
        let chip_gaming = create_chip("Gaming", false);
        let chip_music = create_chip("Music", false);
        let chip_news = create_chip("News", false);
        let add_category_btn = create_chip("+ Add category", false);
        add_category_btn.set_tooltip_text(Some("Add a saved topic search to Home"));

        chips_box.append(&chip_trending);
        chips_box.append(&chip_gaming);
        chips_box.append(&chip_music);
        chips_box.append(&chip_news);

        let chips_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .child(&chips_box)
            .build();
        chips_scroll.add_css_class("yr-no-scrollbar");

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
        subs_scroll.add_css_class("yr-no-scrollbar");
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

        // Playlist Detail / Remote Playlist Header Box
        let playlist_header_box = GtkBox::new(Orientation::Vertical, 10);
        playlist_header_box.set_margin_start(28);
        playlist_header_box.set_margin_end(28);
        playlist_header_box.set_margin_top(12);
        playlist_header_box.set_margin_bottom(16);
        playlist_header_box.set_visible(false);

        let pl_top_row = GtkBox::new(Orientation::Horizontal, 12);
        pl_top_row.set_valign(Align::Center);

        let pl_title_col = GtkBox::new(Orientation::Vertical, 2);
        pl_title_col.set_hexpand(true);

        let playlist_title_lbl = Label::new(Some("Playlist"));
        playlist_title_lbl.add_css_class("yr-h1");
        playlist_title_lbl.set_halign(Align::Start);
        playlist_title_lbl.set_xalign(0.0);
        playlist_title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let playlist_meta_lbl = Label::new(Some(""));
        playlist_meta_lbl.add_css_class("yr-meta");
        playlist_meta_lbl.set_halign(Align::Start);
        playlist_meta_lbl.set_xalign(0.0);
        playlist_meta_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        pl_title_col.append(&playlist_title_lbl);
        pl_title_col.append(&playlist_meta_lbl);

        pl_top_row.append(&pl_title_col);

        let pl_actions_row = GtkBox::new(Orientation::Horizontal, 8);
        pl_actions_row.set_valign(Align::Center);

        let playlist_play_all_btn = Button::with_label("Play all");
        playlist_play_all_btn.add_css_class("yr-btn");
        playlist_play_all_btn.add_css_class("primary");
        set_pointer(&playlist_play_all_btn);

        let playlist_shuffle_btn = Button::with_label("Shuffle");
        playlist_shuffle_btn.add_css_class("yr-btn");
        set_pointer(&playlist_shuffle_btn);

        let playlist_save_remote_btn = Button::with_label("Save playlist");
        playlist_save_remote_btn.add_css_class("yr-btn");
        playlist_save_remote_btn.set_visible(false);
        set_pointer(&playlist_save_remote_btn);

        let playlist_rename_btn = Button::with_label("Rename");
        playlist_rename_btn.add_css_class("yr-chip");
        set_pointer(&playlist_rename_btn);

        let playlist_delete_btn = Button::with_label("Delete");
        playlist_delete_btn.add_css_class("yr-chip");
        set_pointer(&playlist_delete_btn);

        pl_actions_row.append(&playlist_play_all_btn);
        pl_actions_row.append(&playlist_shuffle_btn);
        pl_actions_row.append(&playlist_save_remote_btn);
        pl_actions_row.append(&playlist_rename_btn);
        pl_actions_row.append(&playlist_delete_btn);

        playlist_header_box.append(&pl_top_row);
        playlist_header_box.append(&pl_actions_row);

        // Section Header Row (Title + Search Filters / Clear Watch Later / + New Playlist)
        let section_header_row = GtkBox::new(Orientation::Horizontal, 12);
        section_header_row.set_margin_start(28);
        section_header_row.set_margin_end(28);
        section_header_row.set_margin_top(4);
        section_header_row.set_margin_bottom(12);
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

        let new_playlist_btn = Button::with_label("+ New playlist");
        new_playlist_btn.add_css_class("yr-btn");
        new_playlist_btn.add_css_class("primary");
        new_playlist_btn.set_visible(false);
        set_pointer(&new_playlist_btn);

        section_header_row.append(&section_title);
        section_header_row.append(&search_filters_btn);
        section_header_row.append(&clear_watch_later_btn);
        section_header_row.append(&new_playlist_btn);

        // Search Type Tabs Row (All | Videos | Channels | Playlists)
        let search_tabs_box = GtkBox::new(Orientation::Horizontal, 8);
        search_tabs_box.set_margin_start(28);
        search_tabs_box.set_margin_end(28);
        search_tabs_box.set_margin_bottom(14);

        let stab_all = create_chip("All", true);
        let stab_videos = create_chip("Videos", false);
        let stab_shorts = create_chip("Shorts", false);
        let stab_channels = create_chip("Channels", false);
        let stab_playlists = create_chip("Playlists", false);

        search_tabs_box.append(&stab_all);
        search_tabs_box.append(&stab_videos);
        search_tabs_box.append(&stab_shorts);
        search_tabs_box.append(&stab_channels);
        search_tabs_box.append(&stab_playlists);

        let search_tabs_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .child(&search_tabs_box)
            .visible(false)
            .build();
        search_tabs_scroll.add_css_class("yr-no-scrollbar");

        // Full-width Empty / Error Status View
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

        // Adaptive Video / Content Grid
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
        flow_box.set_margin_bottom(36);

        let feed_box = GtkBox::new(Orientation::Vertical, 0);
        feed_box.set_vexpand(true);
        feed_box.append(&chips_scroll);
        feed_box.append(&subs_strip_box);
        feed_box.append(&history_header_box);
        feed_box.append(&playlist_header_box);
        feed_box.append(&section_header_row);
        feed_box.append(&search_tabs_scroll);
        feed_box.append(&status_box);
        feed_box.append(&flow_box);

        // Inline Kalam Preferences View
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
            .focusable(true)
            .build();
        content_scrolled.add_css_class("yr-no-scrollbar");

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
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(2)
                .thread_name("yr-worker")
                .enable_all()
                .build()
                .expect("Failed to initialize Tokio runtime"),
        );
        let engine = Arc::new(Engine::new(cache.root_dir()));
        let image_loader = ImageLoader::new(cache, runtime.clone(), engine.http_client());

        let content_overlay = Overlay::new();
        content_overlay.set_hexpand(true);
        content_overlay.set_vexpand(true);
        content_overlay.set_child(Some(&nav_view));

        let search_suggest_box = GtkBox::new(Orientation::Vertical, 2);
        search_suggest_box.add_css_class("yr-search-dropdown");
        search_suggest_box.set_hexpand(true);

        let search_dropdown_clamp = adw::Clamp::builder()
            .maximum_size(440)
            .tightening_threshold(240)
            .hexpand(true)
            .halign(Align::Fill)
            .valign(Align::Start)
            .margin_top(6)
            .margin_start(16)
            .margin_end(16)
            .visible(false)
            .child(&search_suggest_box)
            .build();
        content_overlay.add_overlay(&search_dropdown_clamp);

        let player_card = PlayerCard::new(image_loader.clone());
        content_overlay.add_overlay(&player_card.root);

        // Bottom Navigation Bar for Narrow Breakpoint (<= 600px)
        let bottom_bar = GtkBox::new(Orientation::Horizontal, 0);
        bottom_bar.add_css_class("yr-bottom-bar");
        bottom_bar.set_visible(false);

        let bottom_home = create_bottom_nav_btn(KalamIcon::Home, "Home", true);
        let bottom_subs = create_bottom_nav_btn(KalamIcon::Subscriptions, "Subscriptions", false);
        let bottom_playlists = create_bottom_nav_btn(KalamIcon::Playlists, "Playlists", false);
        let bottom_watch_later = create_bottom_nav_btn(KalamIcon::WatchLater, "Watch Later", false);
        let bottom_hist = create_bottom_nav_btn(KalamIcon::History, "History", false);
        let bottom_settings = create_bottom_nav_btn(KalamIcon::Settings, "Preferences", false);

        bottom_bar.append(&bottom_home);
        bottom_bar.append(&bottom_subs);
        bottom_bar.append(&bottom_playlists);
        bottom_bar.append(&bottom_watch_later);
        bottom_bar.append(&bottom_hist);
        bottom_bar.append(&bottom_settings);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.set_hexpand(true);
        toolbar_view.set_vexpand(true);
        toolbar_view.add_top_bar(&header_bar);
        toolbar_view.add_bottom_bar(&bottom_bar);
        toolbar_view.set_content(Some(&content_overlay));

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
        let search_history_store = Arc::new(SearchHistoryStore::new());
        let playlist_store = Arc::new(PlaylistStore::new());
        let settings_store = Arc::new(SettingsStore::new());
        let subscription_store = Arc::new(SubscriptionStore::new());
        let watch_later_store = Arc::new(WatchLaterStore::new());

        let initial_settings = settings_store.load();
        let initial_quality = Quality::from_setting(&initial_settings.default_quality);
        let initial_home_categories = initial_settings.custom_home_categories.clone();

        let this = Rc::new(Self {
            window,
            back_btn,
            refresh_btn,
            search_entry,
            search_clear_btn,
            search_dropdown_clamp,
            search_suggest_box,
            suggest_items: RefCell::new(Vec::new()),
            suggest_selected_idx: Cell::new(-1),
            suggest_seq: Cell::new(0),
            typed_before_nav: RefCell::new(String::new()),
            suppress_search_clear: Cell::new(false),
            rail,
            bottom_home,
            bottom_subs,
            bottom_playlists,
            bottom_watch_later,
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
            new_playlist_btn,
            search_tabs_scroll,
            search_tabs_box,
            stab_all,
            stab_videos,
            stab_shorts,
            stab_channels,
            stab_playlists,
            playlist_header_box,
            playlist_title_lbl,
            playlist_meta_lbl,
            playlist_play_all_btn,
            playlist_shuffle_btn,
            playlist_save_remote_btn,
            playlist_rename_btn,
            playlist_delete_btn,
            chips_scroll,
            chips_box,
            chip_trending,
            chip_gaming,
            chip_music,
            chip_news,
            add_category_btn,
            custom_category_buttons: RefCell::new(Vec::new()),
            subs_strip_box,
            subs_avatars_row,
            history_header_box,
            toast_overlay,
            player_card,
            mpv_session: RefCell::new(None),
            mpv_player_cmd: RefCell::new(initial_settings.external_player.clone()),
            mpv_became_active: Cell::new(false),
            playback_seq: Cell::new(0),
            mpv_sync_gen: Cell::new(0),
            mpv_acked_gen: Cell::new(0),
            queue_continuation: RefCell::new(None),
            queue_cont_fetching: Cell::new(false),
            playback_quality: Cell::new(initial_quality),
            mpv_pos: Cell::new(0.0),
            mpv_duration: Cell::new(0.0),
            is_narrow: Cell::new(false),
            is_compact: Cell::new(false),
            grid_cols: Cell::new(4),
            shorts_cols: Cell::new(6),
            view_mode: Cell::new(ViewMode::Grid),
            active_view: RefCell::new(ActiveView::HomeTrending),
            nav_history: RefCell::new(Vec::new()),
            suppress_nav_push: Cell::new(false),
            channel_over_transient_home: Cell::new(false),
            current_videos: RefCell::new(Vec::new()),
            current_search_items: RefCell::new(Vec::new()),
            remote_playlist_data: RefCell::new(None),
            feed_cursor: RefCell::new(None),
            trending_cursors: RefCell::new(Vec::new()),
            is_loading_more: Cell::new(false),
            request_seq: Cell::new(0),
            search_tab: Cell::new(SearchTab::All),
            search_date_filter: Cell::new(UploadDateFilter::AnyTime),
            search_dur_filter: Cell::new(DurationFilter::AnyLength),
            search_sort_order: Cell::new(SearchSortOrder::Relevance),
            selected_box: RefCell::new(None),
            selected_action_cb: RefCell::new(None),
            open_channels: RefCell::new(Vec::new()),
            home_pictures: RefCell::new(Vec::new()),
            home_avatars: RefCell::new(Vec::new()),
            home_textures_suspended: Cell::new(false),
            last_home_cull_y: Cell::new(0.0),
            theme_provider,
            image_loader,
            history_store,
            search_history_store,
            playlist_store,
            settings_store,
            subscription_store,
            watch_later_store,
            engine,
            runtime,
        });

        this.load_custom_home_categories(initial_home_categories);
        this.chips_box.append(&this.add_category_btn);
        this.setup_home_categories_popover();

        this.apply_appearance(&initial_settings);
        this.sync_preferences_ui(&initial_settings);
        this.wire_preferences_signals(&pref_clear_cache_btn, &pref_clear_hist_btn);
        this.register_app_actions(app);
        this.setup_breakpoint(&bottom_bar);
        this.setup_search_filters_popover();
        this.setup_search_suggestions_bar();
        this.setup_playlist_popovers();
        this.setup_events(&clear_hist_btn);
        this.defocus_search_bar();
        this.load_trending();

        // Warm YouTube session visitor_data shortly after window paint (MPV spawns lazily on first play)
        let weak_warm = Rc::downgrade(&this);
        glib::timeout_add_local_once(Duration::from_millis(450), move || {
            if let Some(w) = weak_warm.upgrade() {
                let eng = w.engine.clone();
                w.runtime.spawn(async move {
                    eng.warm_startup().await;
                });
            }
        });

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
                 popover > contents, .yr-search-dropdown {{ border-color: alpha(#1C1512, 0.14); box-shadow: 0 10px 28px alpha(black, 0.16); }}
                 .yr-suggest-row:hover, .yr-suggest-row.active {{ background-color: alpha(#1C1512, 0.08); }}
                 .yr-btn {{ background-color: alpha(#1C1512, 0.09); }}
                 .yr-pref-card, .yr-entity-card, .yr-playlist-card {{ background-color: alpha(#1C1512, 0.05); }}
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
        let act_playlists = gio::SimpleAction::new("playlists", None);
        act_playlists.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.clear_search_bar_quietly();
                w.set_active_nav(Some(RailTab::Playlists));
                w.load_playlists_overview();
            }
        });
        app.add_action(&act_playlists);

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
                w.set_active_nav(Some(RailTab::WatchLater));
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
        let act_refresh = gio::SimpleAction::new("refresh", None);
        act_refresh.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.reload_current_context();
            }
        });
        app.add_action(&act_refresh);

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
            &self.playlist_header_box,
            &self.section_header_row,
            &self.search_tabs_box,
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
        let this = self.clone();
        self.with_suppressed_nav_push(|| match av {
            ActiveView::Search(_) => {
                if this.search_tab.get() != SearchTab::Shorts
                    && this.flow_box.is_visible()
                    && !this.current_search_items.borrow().is_empty()
                {
                    let items = this.current_search_items.borrow().clone();
                    this.populate_search_items(items);
                }
            }
            ActiveView::History => {
                this.load_history();
            }
            ActiveView::SavedPlaylistDetail(pid) => {
                this.open_saved_playlist(&pid);
            }
            ActiveView::RemotePlaylist(_) => {
                let pl_opt = this.remote_playlist_data.borrow().clone();
                if let Some(pl) = pl_opt {
                    this.render_remote_playlist_view(&pl);
                }
            }
            _ => {}
        });
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
        self.bottom_playlists.remove_css_class("active");
        self.bottom_watch_later.remove_css_class("active");
        self.bottom_hist.remove_css_class("active");
        self.bottom_settings.remove_css_class("active");

        if let Some(t) = tab {
            match t {
                RailTab::Home => self.bottom_home.add_css_class("active"),
                RailTab::Subscriptions => self.bottom_subs.add_css_class("active"),
                RailTab::Playlists => self.bottom_playlists.add_css_class("active"),
                RailTab::WatchLater => self.bottom_watch_later.add_css_class("active"),
                RailTab::History => self.bottom_hist.add_css_class("active"),
                RailTab::Settings => self.bottom_settings.add_css_class("active"),
            }
        }
    }

    fn load_custom_home_categories(self: &Rc<Self>, categories: Vec<String>) {
        let mut seen = HashSet::new();
        for category in categories {
            let category = category.trim().to_string();
            let key = category.to_ascii_lowercase();
            if category.is_empty()
                || category.chars().count() > 40
                || ["trending", "gaming", "music", "news"]
                    .iter()
                    .any(|builtin| builtin.eq_ignore_ascii_case(&category))
                || !seen.insert(key)
            {
                continue;
            }
            self.add_custom_home_category_chip(category);
        }
    }

    fn add_custom_home_category_chip(self: &Rc<Self>, category: String) {
        if self
            .custom_category_buttons
            .borrow()
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(&category))
        {
            return;
        }

        let button = create_chip(&category, false);
        button.set_tooltip_text(Some(
            "Search this topic; right-click to remove the custom category",
        ));
        let after = self
            .custom_category_buttons
            .borrow()
            .last()
            .map(|(_, button)| button.clone())
            .unwrap_or_else(|| self.chip_news.clone());
        self.chips_box.insert_child_after(&button, Some(&after));

        let category_for_search = category.clone();
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |btn| {
            if let Some(win) = weak.upgrade() {
                win.highlight_chip(Some(btn));
                win.search_category(&category_for_search);
            }
        });

        let remove_popover = Popover::new();
        remove_popover.set_parent(&button);
        remove_popover.set_has_arrow(false);
        let remove_btn = Button::with_label("Remove category");
        remove_btn.add_css_class("flat");
        remove_btn.add_css_class("yr-menu-item");
        set_pointer(&remove_btn);
        remove_popover.set_child(Some(&remove_btn));
        let right_click = GestureClick::new();
        right_click.set_button(3);
        let remove_popover_weak = remove_popover.downgrade();
        right_click.connect_pressed(move |_, _, _, _| {
            if let Some(popover) = remove_popover_weak.upgrade() {
                popover.popup();
            }
        });
        button.add_controller(right_click);

        let category_for_remove = category.clone();
        let weak = Rc::downgrade(self);
        let remove_popover_weak = remove_popover.downgrade();
        remove_btn.connect_clicked(move |_| {
            if let Some(popover) = remove_popover_weak.upgrade() {
                popover.popdown();
            }
            if let Some(win) = weak.upgrade() {
                win.remove_custom_home_category(&category_for_remove);
            }
        });

        self.custom_category_buttons
            .borrow_mut()
            .push((category, button));
    }

    fn remove_custom_home_category(self: &Rc<Self>, category: &str) {
        let button = self
            .custom_category_buttons
            .borrow()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(category))
            .map(|(_, button)| button.clone());
        if let Some(button) = button {
            self.chips_box.remove(&button);
        }
        self.custom_category_buttons
            .borrow_mut()
            .retain(|(name, _)| !name.eq_ignore_ascii_case(category));

        let mut settings = self.settings_store.load();
        settings
            .custom_home_categories
            .retain(|name| !name.eq_ignore_ascii_case(category));
        self.settings_store.save(&settings);
        self.highlight_chip(None);
    }

    fn setup_home_categories_popover(self: &Rc<Self>) {
        let popover = Popover::new();
        popover.set_parent(&self.add_category_btn);
        popover.set_has_arrow(false);

        let content = GtkBox::new(Orientation::Vertical, 8);
        content.set_width_request(280);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);

        let heading = Label::new(Some("Add a topic search"));
        heading.add_css_class("yr-popover-head");
        heading.set_halign(Align::Start);
        content.append(&heading);

        let explanation = Label::new(Some("This adds a saved YouTube search to your Home topics."));
        explanation.add_css_class("yr-meta");
        explanation.set_wrap(true);
        explanation.set_halign(Align::Start);
        explanation.set_xalign(0.0);
        content.append(&explanation);

        let entry = Entry::builder()
            .placeholder_text("For example: Science")
            .build();
        entry.add_css_class("yr-search");
        content.append(&entry);

        let add_btn = Button::with_label("Add category");
        add_btn.add_css_class("yr-btn");
        add_btn.add_css_class("primary");
        set_pointer(&add_btn);
        content.append(&add_btn);
        popover.set_child(Some(&content));
        let popover_weak = popover.downgrade();
        self.add_category_btn.connect_clicked(move |_| {
            if let Some(popover) = popover_weak.upgrade() {
                popover.popup();
            }
        });

        let weak = Rc::downgrade(self);
        let popover_weak = popover.downgrade();
        let entry_for_add = entry.clone();
        let add_category = Rc::new(move || {
            let category = entry_for_add.text().trim().to_string();
            let Some(win) = weak.upgrade() else {
                return;
            };
            if category.is_empty() {
                win.toast_overlay
                    .add_toast(adw::Toast::new("Enter a topic first"));
                return;
            }
            if category.chars().count() > 40 {
                win.toast_overlay
                    .add_toast(adw::Toast::new("Keep home topic names under 40 characters"));
                return;
            }
            let duplicate = ["Trending", "Gaming", "Music", "News"]
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&category))
                || win
                    .custom_category_buttons
                    .borrow()
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(&category));
            if duplicate {
                win.toast_overlay
                    .add_toast(adw::Toast::new("That home topic already exists"));
                return;
            }

            let mut settings = win.settings_store.load();
            settings.custom_home_categories.push(category.clone());
            win.settings_store.save(&settings);
            win.add_custom_home_category_chip(category);
            entry_for_add.set_text("");
            if let Some(popover) = popover_weak.upgrade() {
                popover.popdown();
            }
        });
        let add_click = add_category.clone();
        add_btn.connect_clicked(move |_| add_click());
        entry.connect_activate(move |_| add_category());
    }

    fn highlight_chip(&self, target: Option<&Button>) {
        for c in [
            &self.chip_trending,
            &self.chip_gaming,
            &self.chip_music,
            &self.chip_news,
        ] {
            c.remove_css_class("active");
        }
        for (_, button) in self.custom_category_buttons.borrow().iter() {
            button.remove_css_class("active");
        }
        if let Some(btn) = target {
            btn.add_css_class("active");
        }
    }

    fn highlight_search_tab(&self, tab: SearchTab) {
        for (t, btn) in [
            (SearchTab::All, &self.stab_all),
            (SearchTab::Videos, &self.stab_videos),
            (SearchTab::Shorts, &self.stab_shorts),
            (SearchTab::Channels, &self.stab_channels),
            (SearchTab::Playlists, &self.stab_playlists),
        ] {
            if t == tab {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }
    }

    fn update_search_clear_icon(&self) {
        self.search_clear_btn
            .set_visible(!self.search_entry.text().is_empty());
    }

    fn hide_search_dropdown(&self) {
        self.search_dropdown_clamp.set_visible(false);
        self.suggest_selected_idx.set(-1);
    }

    fn set_search_text_quietly(&self, text: &str) {
        self.suppress_search_clear.set(true);
        self.search_entry.set_text(text);
        self.search_entry.set_position(-1);
        self.update_search_clear_icon();
        self.suppress_search_clear.set(false);
    }

    fn clear_search_bar_quietly(&self) {
        self.hide_search_dropdown();
        *self.typed_before_nav.borrow_mut() = String::new();
        if !self.search_entry.text().is_empty() {
            self.suppress_search_clear.set(true);
            self.search_entry.set_text("");
            self.update_search_clear_icon();
            self.suppress_search_clear.set(false);
        }
    }

    fn select_card(&self, target: &GtkBox, action: Rc<dyn Fn()>) {
        if let Some(prev) = self.selected_box.borrow_mut().replace(target.clone()) {
            if prev != *target {
                prev.remove_css_class("selected");
            }
        }
        target.add_css_class("selected");
        *self.selected_action_cb.borrow_mut() = Some(action);
    }

    fn clear_card_selection(&self) {
        if let Some(prev) = self.selected_box.borrow_mut().take() {
            prev.remove_css_class("selected");
        }
        *self.selected_action_cb.borrow_mut() = None;
    }

    fn update_back_button_visibility(&self) {
        let on_subpage = self
            .nav_view
            .visible_page()
            .and_then(|p| p.tag())
            .map(|t| t.as_str() != "home")
            .unwrap_or(false);
        let has_history = !self.nav_history.borrow().is_empty();
        self.back_btn.set_visible(on_subpage || has_history);
    }

    fn reset_nav_root<F: FnOnce()>(&self, action: F) {
        self.nav_history.borrow_mut().clear();
        self.channel_over_transient_home.set(false);
        let prev = self.suppress_nav_push.replace(true);
        action();
        self.suppress_nav_push.set(prev);
        self.update_back_button_visibility();
    }

    fn with_suppressed_nav_push<F: FnOnce()>(&self, action: F) {
        let prev = self.suppress_nav_push.replace(true);
        action();
        self.suppress_nav_push.set(prev);
        self.update_back_button_visibility();
    }

    fn defocus_search_bar(&self) {
        self.hide_search_dropdown();
        self.search_entry.select_region(0, 0);
        gtk4::prelude::GtkWindowExt::set_focus(&self.window, Some(&self.content_scrolled));
    }

    fn go_back(self: &Rc<Self>) {
        self.defocus_search_bar();
        let on_subpage = self
            .nav_view
            .visible_page()
            .and_then(|p| p.tag())
            .map(|t| t.as_str() != "home")
            .unwrap_or(false);
        if on_subpage {
            let single_channel = self.open_channels.borrow().len() <= 1;
            self.nav_view.pop();
            if single_channel && self.channel_over_transient_home.replace(false) {
                let entry_opt = self.nav_history.borrow_mut().pop();
                if let Some(entry) = entry_opt {
                    self.restore_nav_entry(entry);
                }
            }
            self.update_back_button_visibility();
            self.defocus_search_bar();
            return;
        }

        let entry_opt = self.nav_history.borrow_mut().pop();
        if let Some(entry) = entry_opt {
            self.restore_nav_entry(entry);
        }
        self.update_back_button_visibility();
        self.defocus_search_bar();
    }

    fn restore_nav_entry(self: &Rc<Self>, entry: NavHistoryEntry) {
        match entry {
            NavHistoryEntry::Channel { id, name } => {
                self.channel_over_transient_home.set(true);
                self.open_channel(&id, &name);
            }
            NavHistoryEntry::MainView {
                view,
                search_tab,
                search_date,
                search_dur,
                search_sort,
            } => {
                self.channel_over_transient_home.set(false);
                if let ActiveView::Search(ref q) = view {
                    self.search_tab.set(search_tab);
                    self.search_date_filter.set(search_date);
                    self.search_dur_filter.set(search_dur);
                    self.search_sort_order.set(search_sort);
                    self.set_search_text_quietly(q);
                    let q_owned = q.clone();
                    let this = self.clone();
                    self.with_suppressed_nav_push(|| {
                        this.search(&q_owned);
                    });
                    return;
                }

                self.clear_search_bar_quietly();
                let this = self.clone();
                self.with_suppressed_nav_push(|| match view {
                    ActiveView::HomeCategory(cat) => {
                        this.set_active_nav(Some(RailTab::Home));
                        let button = match cat.as_str() {
                            "Gaming" => Some(this.chip_gaming.clone()),
                            "Music" => Some(this.chip_music.clone()),
                            "News" => Some(this.chip_news.clone()),
                            _ => this
                                .custom_category_buttons
                                .borrow()
                                .iter()
                                .find(|(name, _)| name.eq_ignore_ascii_case(&cat))
                                .map(|(_, button)| button.clone())
                                .or_else(|| Some(this.chip_trending.clone())),
                        };
                        this.highlight_chip(button.as_ref());
                        this.search_category(&cat);
                    }
                    ActiveView::WatchLater => {
                        this.set_active_nav(Some(RailTab::WatchLater));
                        this.load_watch_later();
                    }
                    ActiveView::Subscriptions => {
                        this.set_active_nav(Some(RailTab::Subscriptions));
                        this.load_subscriptions();
                    }
                    ActiveView::Playlists => {
                        this.set_active_nav(Some(RailTab::Playlists));
                        this.load_playlists_overview();
                    }
                    ActiveView::SavedPlaylistDetail(pid) => {
                        this.set_active_nav(Some(RailTab::Playlists));
                        this.open_saved_playlist(&pid);
                    }
                    ActiveView::RemotePlaylist(pid) => {
                        this.open_remote_playlist(&pid);
                    }
                    ActiveView::History => {
                        this.set_active_nav(Some(RailTab::History));
                        this.load_history();
                    }
                    ActiveView::Preferences => {
                        this.open_settings();
                    }
                    ActiveView::HomeTrending | ActiveView::Search(_) => {
                        this.set_active_nav(Some(RailTab::Home));
                        this.highlight_chip(Some(&this.chip_trending));
                        this.load_trending();
                    }
                });
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
            ViewMode::ShortsGrid => {
                let scols = self.shorts_cols.get();
                self.flow_box.set_homogeneous(true);
                self.flow_box.set_min_children_per_line(scols);
                self.flow_box.set_max_children_per_line(scols);
                self.flow_box.set_column_spacing(14);
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

    fn setup_search_suggestions_bar(self: &Rc<Self>) {
        // Separate overlay button gives the clear X a precise circular hover target.
        let weak_clear = Rc::downgrade(self);
        self.search_clear_btn.connect_clicked(move |_| {
            if let Some(w) = weak_clear.upgrade() {
                w.set_search_text_quietly("");
                *w.typed_before_nav.borrow_mut() = String::new();
                w.search_entry.grab_focus();
                w.refresh_search_dropdown();
            }
        });

        // Show recent search history or current suggestions when focusing the search bar
        let focus_ctrl = EventControllerFocus::new();
        let weak_enter = Rc::downgrade(self);
        focus_ctrl.connect_enter(move |_| {
            if let Some(w) = weak_enter.upgrade() {
                *w.typed_before_nav.borrow_mut() = w.search_entry.text().to_string();
                w.refresh_search_dropdown();
            }
        });
        self.search_entry.add_controller(focus_ctrl);

        // Unfocus the search bar and dismiss suggestions whenever clicking anywhere outside it
        let outside_click = GestureClick::new();
        outside_click.set_propagation_phase(PropagationPhase::Capture);
        let weak_out = Rc::downgrade(self);
        outside_click.connect_pressed(move |_, _, x, y| {
            let Some(w) = weak_out.upgrade() else {
                return;
            };
            let pt = gtk4::graphene::Point::new(x as f32, y as f32);
            if let Some(bounds) = w.search_entry.compute_bounds(&w.toast_overlay) {
                if bounds.contains_point(&pt) {
                    return;
                }
            }
            if w.search_dropdown_clamp.is_visible() {
                if let Some(bounds) = w.search_dropdown_clamp.compute_bounds(&w.toast_overlay) {
                    if bounds.contains_point(&pt) {
                        return;
                    }
                }
            }

            let in_search = gtk4::prelude::GtkWindowExt::focus(&w.window)
                .map(|f| {
                    f.is_ancestor(&w.search_entry)
                        || &f == w.search_entry.upcast_ref::<gtk4::Widget>()
                })
                .unwrap_or(false);

            if w.search_dropdown_clamp.is_visible() || in_search {
                w.defocus_search_bar();
            }
        });
        self.toast_overlay.add_controller(outside_click);

        // Up / Down / Escape keyboard navigation inside Search Entry
        let key_ctrl = EventControllerKey::new();
        key_ctrl.set_propagation_phase(PropagationPhase::Capture);
        let weak_key = Rc::downgrade(self);
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            let Some(w) = weak_key.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Down => {
                    if !w.search_dropdown_clamp.is_visible() {
                        w.refresh_search_dropdown();
                    }
                    w.step_suggestion_selection(1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    if w.search_dropdown_clamp.is_visible() {
                        w.step_suggestion_selection(-1);
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                }
                gdk::Key::Escape => {
                    if w.search_dropdown_clamp.is_visible() {
                        w.hide_search_dropdown();
                        return glib::Propagation::Stop;
                    }
                    w.defocus_search_bar();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.search_entry.add_controller(key_ctrl);
    }

    fn step_suggestion_selection(&self, delta: isize) {
        let items = self.suggest_items.borrow();
        let len = items.len() as isize;
        if len == 0 {
            return;
        }
        let cur = self.suggest_selected_idx.get();
        if cur == -1 && delta > 0 {
            *self.typed_before_nav.borrow_mut() = self.search_entry.text().to_string();
        }
        let next = (cur + delta).clamp(-1, len - 1);
        self.suggest_selected_idx.set(next);

        for (i, (_, row)) in items.iter().enumerate() {
            if i as isize == next {
                row.add_css_class("active");
            } else {
                row.remove_css_class("active");
            }
        }

        if next >= 0 {
            if let Some((txt, _)) = items.get(next as usize) {
                self.set_search_text_quietly(txt);
            }
        } else {
            let draft = self.typed_before_nav.borrow().clone();
            self.set_search_text_quietly(&draft);
        }
    }

    fn refresh_search_dropdown(self: &Rc<Self>) {
        let typed = self.search_entry.text().trim().to_string();
        let seq = self.suggest_seq.get().wrapping_add(1);
        self.suggest_seq.set(seq);

        if typed.is_empty() {
            let history = self.search_history_store.load_all();
            self.render_history_dropdown(&history);
            return;
        }

        if looks_like_youtube_url(&typed) {
            self.render_url_hint_dropdown(&typed);
            return;
        }

        // Immediate local history matches without hiding/blinking if remote suggestions are in flight
        let lower = typed.to_lowercase();
        let local_matches: Vec<String> = self
            .search_history_store
            .load_all()
            .into_iter()
            .filter(|h| h.to_lowercase().contains(&lower))
            .take(3)
            .collect();
        if !local_matches.is_empty() {
            self.render_suggestions_dropdown(&local_matches, &[]);
        }

        let weak = Rc::downgrade(self);
        let query_owned = typed;
        glib::timeout_add_local_once(Duration::from_millis(160), move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.suggest_seq.get() != seq {
                return;
            }
            let engine = w.engine.clone();
            let q = query_owned.clone();
            let (tx, rx) = async_channel::bounded::<Result<Vec<String>, String>>(1);
            w.runtime.spawn(async move {
                let res = engine.search_suggestions(&q).await;
                let _ = tx.send(res).await;
            });

            let weak2 = Rc::downgrade(&w);
            glib::spawn_future_local(async move {
                let Ok(Ok(remote)) = rx.recv().await else {
                    return;
                };
                let Some(win) = weak2.upgrade() else {
                    return;
                };
                if win.suggest_seq.get() != seq {
                    return;
                }
                let search_focused = gtk4::prelude::GtkWindowExt::focus(&win.window)
                    .map(|f| {
                        f.is_ancestor(&win.search_entry)
                            || &f == win.search_entry.upcast_ref::<gtk4::Widget>()
                    })
                    .unwrap_or(false);
                if !search_focused {
                    return;
                }
                let now_lower = win.search_entry.text().trim().to_lowercase();
                let hist: Vec<String> = win
                    .search_history_store
                    .load_all()
                    .into_iter()
                    .filter(|h| h.to_lowercase().contains(&now_lower))
                    .take(3)
                    .collect();
                win.render_suggestions_dropdown(&hist, &remote);
            });
        });
    }

    fn clear_suggest_box(&self) {
        while let Some(child) = self.search_suggest_box.first_child() {
            self.search_suggest_box.remove(&child);
        }
        self.suggest_items.borrow_mut().clear();
        self.suggest_selected_idx.set(-1);
    }

    fn render_url_hint_dropdown(self: &Rc<Self>, url: &str) {
        self.clear_suggest_box();

        let head = Label::new(Some("YOUTUBE LINK DETECTED"));
        head.add_css_class("yr-popover-head");
        head.set_halign(Align::Start);
        self.search_suggest_box.append(&head);

        let row = GtkBox::new(Orientation::Horizontal, 8);
        row.add_css_class("yr-suggest-row");
        let btn = Button::builder()
            .label("Open YouTube link directly ↵")
            .hexpand(true)
            .build();
        btn.set_can_focus(false);
        btn.set_focus_on_click(false);
        btn.add_css_class("flat");
        set_pointer(&btn);
        if let Some(lbl) = btn.child().and_then(|c| c.downcast::<Label>().ok()) {
            lbl.set_halign(Align::Start);
            lbl.set_xalign(0.0);
        }

        let weak = Rc::downgrade(self);
        let url_owned = url.to_string();
        btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.hide_search_dropdown();
                gtk4::prelude::GtkWindowExt::set_focus(&w.window, gtk4::Widget::NONE);
                w.submit_search_or_url(&url_owned);
            }
        });
        row.append(&btn);
        self.search_suggest_box.append(&row);
        self.suggest_items
            .borrow_mut()
            .push((url.to_string(), row));
        self.search_dropdown_clamp.set_visible(true);
    }

    fn render_history_dropdown(self: &Rc<Self>, history: &[String]) {
        self.clear_suggest_box();
        if history.is_empty() {
            self.hide_search_dropdown();
            return;
        }

        let header_row = GtkBox::new(Orientation::Horizontal, 8);
        header_row.set_margin_start(10);
        header_row.set_margin_end(6);
        header_row.set_margin_top(4);

        let head_lbl = Label::new(Some("RECENT SEARCHES"));
        head_lbl.add_css_class("yr-popover-head");
        head_lbl.set_margin_start(0);
        head_lbl.set_margin_top(0);
        head_lbl.set_hexpand(true);
        head_lbl.set_halign(Align::Start);

        let clear_all_btn = Button::with_label("Clear all");
        clear_all_btn.set_can_focus(false);
        clear_all_btn.set_focus_on_click(false);
        clear_all_btn.add_css_class("flat");
        clear_all_btn.add_css_class("yr-link");
        set_pointer(&clear_all_btn);
        let weak_clear = Rc::downgrade(self);
        clear_all_btn.connect_clicked(move |_| {
            if let Some(w) = weak_clear.upgrade() {
                w.search_history_store.clear();
                w.hide_search_dropdown();
                w.search_entry.grab_focus();
            }
        });

        header_row.append(&head_lbl);
        header_row.append(&clear_all_btn);
        self.search_suggest_box.append(&header_row);

        for item in history.iter().take(8) {
            let row = GtkBox::new(Orientation::Horizontal, 4);
            row.add_css_class("yr-suggest-row");

            let icon = Image::from_icon_name("document-open-recent-symbolic");
            icon.add_css_class("yr-meta");
            icon.set_margin_start(6);
            row.append(&icon);

            let pick_btn = Button::builder().label(item).hexpand(true).build();
            pick_btn.set_can_focus(false);
            pick_btn.set_focus_on_click(false);
            pick_btn.add_css_class("flat");
            set_pointer(&pick_btn);
            if let Some(lbl) = pick_btn.child().and_then(|c| c.downcast::<Label>().ok()) {
                lbl.set_halign(Align::Start);
                lbl.set_xalign(0.0);
                lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            }

            let weak_pick = Rc::downgrade(self);
            let q_pick = item.clone();
            pick_btn.connect_clicked(move |_| {
                if let Some(w) = weak_pick.upgrade() {
                    w.set_search_text_quietly(&q_pick);
                    *w.typed_before_nav.borrow_mut() = q_pick.clone();
                    w.hide_search_dropdown();
                    gtk4::prelude::GtkWindowExt::set_focus(&w.window, gtk4::Widget::NONE);
                    w.submit_search_or_url(&q_pick);
                }
            });

            let rm_btn = Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove from search history")
                .build();
            rm_btn.set_can_focus(false);
            rm_btn.set_focus_on_click(false);
            rm_btn.add_css_class("flat");
            rm_btn.add_css_class("circular");
            set_pointer(&rm_btn);
            let weak_rm = Rc::downgrade(self);
            let q_rm = item.clone();
            rm_btn.connect_clicked(move |_| {
                if let Some(w) = weak_rm.upgrade() {
                    w.search_history_store.remove(&q_rm);
                    let updated = w.search_history_store.load_all();
                    w.render_history_dropdown(&updated);
                }
            });

            row.append(&pick_btn);
            row.append(&rm_btn);
            self.search_suggest_box.append(&row);
            self.suggest_items
                .borrow_mut()
                .push((item.clone(), row));
        }

        self.search_dropdown_clamp.set_visible(true);
    }

    fn render_suggestions_dropdown(self: &Rc<Self>, local_hist: &[String], remote: &[String]) {
        self.clear_suggest_box();

        let mut seen = HashSet::new();
        let mut combined: Vec<(String, bool)> = Vec::new();
        for h in local_hist {
            if seen.insert(h.to_lowercase()) {
                combined.push((h.clone(), true));
            }
        }
        for r in remote {
            if seen.insert(r.to_lowercase()) {
                combined.push((r.clone(), false));
                if combined.len() >= 8 {
                    break;
                }
            }
        }

        if combined.is_empty() {
            self.hide_search_dropdown();
            return;
        }

        for (text, is_hist) in combined {
            let row = GtkBox::new(Orientation::Horizontal, 6);
            row.add_css_class("yr-suggest-row");

            let icon = Image::from_icon_name(if is_hist {
                "document-open-recent-symbolic"
            } else {
                "system-search-symbolic"
            });
            icon.add_css_class("yr-meta");
            icon.set_margin_start(6);
            row.append(&icon);

            let btn = Button::builder().label(&text).hexpand(true).build();
            btn.set_can_focus(false);
            btn.set_focus_on_click(false);
            btn.add_css_class("flat");
            set_pointer(&btn);
            if let Some(lbl) = btn.child().and_then(|c| c.downcast::<Label>().ok()) {
                lbl.set_halign(Align::Start);
                lbl.set_xalign(0.0);
                lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            }

            let weak_pick = Rc::downgrade(self);
            let q_pick = text.clone();
            btn.connect_clicked(move |_| {
                if let Some(w) = weak_pick.upgrade() {
                    w.set_search_text_quietly(&q_pick);
                    *w.typed_before_nav.borrow_mut() = q_pick.clone();
                    w.hide_search_dropdown();
                    gtk4::prelude::GtkWindowExt::set_focus(&w.window, gtk4::Widget::NONE);
                    w.submit_search_or_url(&q_pick);
                }
            });
            row.append(&btn);

            if is_hist {
                let rm_btn = Button::builder()
                    .icon_name("user-trash-symbolic")
                    .tooltip_text("Remove from search history")
                    .build();
                rm_btn.set_can_focus(false);
                rm_btn.set_focus_on_click(false);
                rm_btn.add_css_class("flat");
                rm_btn.add_css_class("circular");
                set_pointer(&rm_btn);
                let weak_rm = Rc::downgrade(self);
                let q_rm = text.clone();
                rm_btn.connect_clicked(move |_| {
                    if let Some(w) = weak_rm.upgrade() {
                        w.search_history_store.remove(&q_rm);
                        w.refresh_search_dropdown();
                    }
                });
                row.append(&rm_btn);
            }

            self.search_suggest_box.append(&row);
            self.suggest_items.borrow_mut().push((text, row));
        }

        self.search_dropdown_clamp.set_visible(true);
    }

    fn setup_playlist_popovers(self: &Rc<Self>) {
        // "+ New playlist" popover on Playlists overview
        let new_pop = Popover::new();
        new_pop.set_parent(&self.new_playlist_btn);
        new_pop.set_has_arrow(false);
        let np_clean = new_pop.clone();
        self.new_playlist_btn.connect_destroy(move |_| {
            np_clean.unparent();
        });

        let np_box = GtkBox::new(Orientation::Vertical, 6);
        np_box.set_width_request(240);
        np_box.set_margin_top(4);
        np_box.set_margin_bottom(4);
        np_box.set_margin_start(4);
        np_box.set_margin_end(4);

        let np_head = Label::new(Some("NEW PLAYLIST"));
        np_head.add_css_class("yr-popover-head");
        np_head.set_halign(Align::Start);
        np_box.append(&np_head);

        let np_entry = Entry::builder()
            .placeholder_text("Playlist name…")
            .build();
        np_entry.add_css_class("yr-search");
        np_box.append(&np_entry);

        let np_create = Button::with_label("Create playlist");
        np_create.add_css_class("yr-btn");
        np_create.add_css_class("primary");
        set_pointer(&np_create);
        np_box.append(&np_create);
        new_pop.set_child(Some(&np_box));

        let weak_np = Rc::downgrade(self);
        let pop_np_weak = new_pop.downgrade();
        let np_entry_cl = np_entry.clone();
        let do_create = Rc::new(move || {
            let name = np_entry_cl.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            if let Some(w) = weak_np.upgrade() {
                let pl = w.playlist_store.create(&name);
                np_entry_cl.set_text("");
                w.toast_overlay
                    .add_toast(adw::Toast::new(&format!("Created “{}”", pl.name)));
                w.load_playlists_overview();
            }
            if let Some(p) = pop_np_weak.upgrade() {
                p.popdown();
            }
        });
        let dc_btn = do_create.clone();
        np_create.connect_clicked(move |_| dc_btn());
        np_entry.connect_activate(move |_| do_create());

        let np_open = new_pop.downgrade();
        let np_entry_focus = np_entry;
        self.new_playlist_btn.connect_clicked(move |_| {
            if let Some(p) = np_open.upgrade() {
                p.popup();
                np_entry_focus.grab_focus();
            }
        });

        // "Rename" popover on SavedPlaylistDetail view
        let ren_pop = Popover::new();
        ren_pop.set_parent(&self.playlist_rename_btn);
        ren_pop.set_has_arrow(false);
        let rp_clean = ren_pop.clone();
        self.playlist_rename_btn.connect_destroy(move |_| {
            rp_clean.unparent();
        });

        let ren_box = GtkBox::new(Orientation::Vertical, 6);
        ren_box.set_width_request(240);
        ren_box.set_margin_top(4);
        ren_box.set_margin_bottom(4);
        ren_box.set_margin_start(4);
        ren_box.set_margin_end(4);

        let ren_head = Label::new(Some("RENAME PLAYLIST"));
        ren_head.add_css_class("yr-popover-head");
        ren_head.set_halign(Align::Start);
        ren_box.append(&ren_head);

        let ren_entry = Entry::builder()
            .placeholder_text("New playlist name…")
            .build();
        ren_entry.add_css_class("yr-search");
        ren_box.append(&ren_entry);

        let ren_save = Button::with_label("Save name");
        ren_save.add_css_class("yr-btn");
        ren_save.add_css_class("primary");
        set_pointer(&ren_save);
        ren_box.append(&ren_save);
        ren_pop.set_child(Some(&ren_box));

        let weak_ren = Rc::downgrade(self);
        let pop_ren_weak = ren_pop.downgrade();
        let ren_entry_cl = ren_entry.clone();
        let do_rename = Rc::new(move || {
            let new_name = ren_entry_cl.text().trim().to_string();
            if new_name.is_empty() {
                return;
            }
            if let Some(w) = weak_ren.upgrade() {
                let pid_opt = match &*w.active_view.borrow() {
                    ActiveView::SavedPlaylistDetail(pid) => Some(pid.clone()),
                    _ => None,
                };
                if let Some(pid) = pid_opt {
                    if w.playlist_store.rename(&pid, &new_name) {
                        w.open_saved_playlist(&pid);
                        w.toast_overlay
                            .add_toast(adw::Toast::new("Renamed playlist"));
                    }
                }
            }
            if let Some(p) = pop_ren_weak.upgrade() {
                p.popdown();
            }
        });
        let dr_btn = do_rename.clone();
        ren_save.connect_clicked(move |_| dr_btn());
        ren_entry.connect_activate(move |_| do_rename());

        let rp_open = ren_pop.downgrade();
        let weak_prefill = Rc::downgrade(self);
        let ren_entry_open = ren_entry;
        self.playlist_rename_btn.connect_clicked(move |_| {
            if let Some(w) = weak_prefill.upgrade() {
                if let ActiveView::SavedPlaylistDetail(pid) = &*w.active_view.borrow() {
                    if let Some(pl) = w.playlist_store.get(pid) {
                        ren_entry_open.set_text(&pl.name);
                    }
                }
            }
            if let Some(p) = rp_open.upgrade() {
                p.popup();
                ren_entry_open.grab_focus();
            }
        });
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

        // Off-screen texture culling + near-bottom smart infinite scroll
        let weak_scroll = Rc::downgrade(self);
        self.content_scrolled
            .vadjustment()
            .connect_value_changed(move |adj| {
                let Some(w) = weak_scroll.upgrade() else {
                    return;
                };
                w.maybe_cull_home_textures();
                let val = adj.value();
                let page = adj.page_size();
                let upper = adj.upper();
                if val > 0.0 && upper > page && val + page >= upper - 420.0 {
                    w.load_more_current_feed();
                }
            });

        let weak_refresh = Rc::downgrade(self);
        self.refresh_btn.connect_clicked(move |_| {
            if let Some(w) = weak_refresh.upgrade() {
                w.reload_current_context();
            }
        });

        // Window keyboard shortcuts: '/', 'Ctrl+F', 'Ctrl+R'/'F5', 'Alt+Left', 'Esc', 'Enter' on selected card
        let key_ctrl = EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };

            if (mods.contains(gdk::ModifierType::CONTROL_MASK)
                && (key == gdk::Key::r || key == gdk::Key::R))
                || key == gdk::Key::F5
            {
                w.reload_current_context();
                return glib::Propagation::Stop;
            }

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

            let in_editable = gtk4::prelude::GtkWindowExt::focus(&w.window)
                .map(|f| f.is::<Editable>() || f.ancestor(Editable::static_type()).is_some())
                .unwrap_or(false);

            if key == gdk::Key::slash
                && !in_editable
                && !mods.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                )
            {
                w.search_entry.grab_focus();
                return glib::Propagation::Stop;
            }

            if (key == gdk::Key::Return || key == gdk::Key::KP_Enter) && !in_editable {
                let on_home = w
                    .nav_view
                    .visible_page()
                    .and_then(|p| p.tag())
                    .map(|t| t.as_str() == "home")
                    .unwrap_or(true);
                if on_home {
                    let cb_opt = w.selected_action_cb.borrow().clone();
                    if let Some(cb) = cb_opt {
                        cb();
                        return glib::Propagation::Stop;
                    }
                }
            }

            if key == gdk::Key::Escape {
                let search_focused = gtk4::prelude::GtkWindowExt::focus(&w.window)
                    .map(|f| {
                        f.is_ancestor(&w.search_entry)
                            || &f == w.search_entry.upcast_ref::<gtk4::Widget>()
                    })
                    .unwrap_or(false);
                if search_focused || w.search_dropdown_clamp.is_visible() {
                    w.defocus_search_bar();
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
            w.defocus_search_bar();
            let is_home = nv
                .visible_page()
                .and_then(|p| p.tag())
                .map(|t| t.as_str() == "home")
                .unwrap_or(true);
            w.update_back_button_visibility();

            if is_home {
                w.open_channels.borrow_mut().clear();
                w.image_loader.schedule_heap_trim();
                if *w.active_view.borrow() == ActiveView::Subscriptions {
                    w.home_textures_suspended.set(false);
                    w.load_subscriptions();
                } else {
                    w.resume_home_textures();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.nav_view.connect_popped(move |_, popped_page| {
            if let Some(w) = weak.upgrade() {
                w.defocus_search_bar();
                w.open_channels
                    .borrow_mut()
                    .retain(|cp| &cp.page != popped_page);
                let top_cp = w.open_channels.borrow().last().cloned();
                if let Some(cp) = top_cp {
                    cp.resume_textures();
                }
                w.image_loader.schedule_heap_trim();
            }
        });

        // Status page action button
        let weak = Rc::downgrade(self);
        self.status_action_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let cb_opt = w.status_action_cb.borrow().clone();
                if let Some(cb) = cb_opt {
                    cb();
                }
            }
        });

        // Search activation + synchronous live suggestions (no delayed phantom signals or page hijacks)
        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |entry| {
            let query = entry.text().trim().to_string();
            if let Some(w) = weak.upgrade() {
                w.defocus_search_bar();
                if !query.is_empty() {
                    w.submit_search_or_url(&query);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.search_entry.connect_changed(move |entry| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            w.update_search_clear_icon();
            if w.suppress_search_clear.get() {
                return;
            }
            *w.typed_before_nav.borrow_mut() = entry.text().to_string();
            let search_focused = gtk4::prelude::GtkWindowExt::focus(&w.window)
                .map(|f| {
                    f.is_ancestor(&w.search_entry)
                        || &f == w.search_entry.upcast_ref::<gtk4::Widget>()
                })
                .unwrap_or(false);
            if search_focused {
                w.refresh_search_dropdown();
            }
        });

        // Search result type tab pills (All / Videos / Shorts / Channels / Playlists)
        for (tab, btn) in [
            (SearchTab::All, &self.stab_all),
            (SearchTab::Videos, &self.stab_videos),
            (SearchTab::Shorts, &self.stab_shorts),
            (SearchTab::Channels, &self.stab_channels),
            (SearchTab::Playlists, &self.stab_playlists),
        ] {
            let weak_tab = Rc::downgrade(self);
            btn.connect_clicked(move |_| {
                if let Some(w) = weak_tab.upgrade() {
                    if w.search_tab.get() != tab {
                        w.search_tab.set(tab);
                        w.highlight_search_tab(tab);
                        let q_opt = match &*w.active_view.borrow() {
                            ActiveView::Search(q) => Some(q.clone()),
                            _ => None,
                        };
                        if let Some(q) = q_opt {
                            w.search(&q);
                        }
                    }
                }
            });
        }

        // Playlist detail header buttons
        let weak = Rc::downgrade(self);
        self.playlist_play_all_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let vids = w.current_videos.borrow().clone();
                if vids.is_empty() {
                    w.toast_overlay
                        .add_toast(adw::Toast::new("This playlist has no videos yet"));
                } else {
                    let q = Quality::from_setting(&w.settings_store.load().default_quality);
                    let cont = w.remote_playlist_queue_continuation();
                    w.play_queue_from_with_continuation(vids, 0, q, false, cont);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.playlist_shuffle_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let mut vids = w.current_videos.borrow().clone();
                if vids.is_empty() {
                    w.toast_overlay
                        .add_toast(adw::Toast::new("This playlist has no videos yet"));
                } else {
                    shuffle_slice(&mut vids);
                    let q = Quality::from_setting(&w.settings_store.load().default_quality);
                    w.play_queue_from(vids, 0, q, false);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.playlist_save_remote_btn.connect_clicked(move |btn| {
            if let Some(w) = weak.upgrade() {
                if let Some(remote) = w.remote_playlist_data.borrow().clone() {
                    let vids = w.current_videos.borrow().clone();
                    let saved = w.playlist_store.create_with_videos(
                        &remote.name,
                        vids,
                        Some(remote.id.clone()),
                        &remote.channel_name,
                    );
                    btn.set_label("Saved ✓");
                    w.toast_overlay
                        .add_toast(adw::Toast::new(&format!("Saved playlist “{}”", saved.name)));
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.playlist_delete_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let pid_opt = match &*w.active_view.borrow() {
                    ActiveView::SavedPlaylistDetail(pid) => Some(pid.clone()),
                    _ => None,
                };
                if let Some(pid) = pid_opt {
                    w.playlist_store.delete(&pid);
                    w.nav_history.borrow_mut().retain(|e| {
                        !matches!(
                            e,
                            NavHistoryEntry::MainView {
                                view: ActiveView::SavedPlaylistDetail(ref saved_id),
                                ..
                            } if saved_id == &pid
                        )
                    });
                    w.toast_overlay
                        .add_toast(adw::Toast::new("Deleted playlist"));
                    let win = w.clone();
                    w.with_suppressed_nav_push(|| {
                        win.set_active_nav(Some(RailTab::Playlists));
                        win.load_playlists_overview();
                    });
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
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::Home));
                    win.highlight_chip(Some(&win.chip_trending));
                    win.load_trending();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.home_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::Home));
                    win.highlight_chip(Some(&win.chip_trending));
                    win.load_trending();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.subs_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::Subscriptions));
                    win.load_subscriptions();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.playlists_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::Playlists));
                    win.load_playlists_overview();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.watch_later_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.navigate_to_watch_later();
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.hist_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::History));
                    win.load_history();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.rail.settings_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.open_settings();
                });
            }
        });

        // Narrow Bottom Bar Navigation
        let weak = Rc::downgrade(self);
        self.bottom_home.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::Home));
                    win.highlight_chip(Some(&win.chip_trending));
                    win.load_trending();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_subs.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::Subscriptions));
                    win.load_subscriptions();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_playlists.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::Playlists));
                    win.load_playlists_overview();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_watch_later.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.navigate_to_watch_later();
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_hist.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.clear_search_bar_quietly();
                    win.set_active_nav(Some(RailTab::History));
                    win.load_history();
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.bottom_settings.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let win = w.clone();
                w.reset_nav_root(|| {
                    win.open_settings();
                });
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

        // Player Card Controls Wiring (App-managed queue -> direct RustyPipe streams)
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
                if w.player_card.advance_next().is_some() {
                    let target = w.player_card.current_idx();
                    w.jump_or_reload_queue_index(target);
                } else {
                    w.toast_overlay
                        .add_toast(adw::Toast::new("End of queue"));
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_prev(move || {
            if let Some(w) = weak.upgrade() {
                if w.player_card.advance_prev().is_some() {
                    let target = w.player_card.current_idx();
                    w.jump_or_reload_queue_index(target);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_jump_to(move |idx| {
            if let Some(w) = weak.upgrade() {
                w.player_card.set_current_index(idx);
                w.jump_or_reload_queue_index(idx);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_queue_mutated(move || {
            if let Some(w) = weak.upgrade() {
                w.resync_mpv_upcoming_queue();
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_save_playlist(move |name, items| {
            if let Some(w) = weak.upgrade() {
                let pl = w.playlist_store.create_with_videos(&name, items, None, "");
                w.toast_overlay
                    .add_toast(adw::Toast::new(&format!("Saved queue as “{}”", pl.name)));
                if *w.active_view.borrow() == ActiveView::Playlists {
                    w.load_playlists_overview();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_empty_stop(move || {
            if let Some(w) = weak.upgrade() {
                w.reset_mpv_queue_tracking();
                w.player_card.set_loading(false);
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.stop();
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
                w.reset_mpv_queue_tracking();
                w.player_card.set_loading(false);
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.stop();
                }
                w.player_card.set_queue(vec![], 0, false);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_card.connect_close(move || {
            if let Some(w) = weak.upgrade() {
                w.reset_mpv_queue_tracking();
                w.player_card.set_loading(false);
                if let Some(session) = w.mpv_session.borrow().as_ref() {
                    session.stop();
                }
                w.player_card.set_queue(vec![], 0, false);
                w.player_card.root.set_visible(false);
            }
        });
    }

    fn sync_preferences_ui(&self, settings: &AppSettings) {
        self.pref_scheme_btn
            .set_label(match settings.color_scheme.as_str() {
                "system" => "System",
                "light" => "Light",
                _ => "Dark",
            });
        self.pref_accent_btn
            .set_label(match settings.accent_color.as_str() {
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
                        let q = Quality::from_setting(key);
                        s.default_quality = q.as_setting().to_string();
                        w.playback_quality.set(q);
                        if let Some(sess) = w.mpv_session.borrow().as_ref() {
                            sess.set_default_quality(q);
                        }
                        w.settings_store.save(&s);
                        w.sync_preferences_ui(&s);
                    }
                }
            },
        );

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

        let store_h = self.settings_store.clone();
        self.pref_hist_switch.connect_state_set(move |_, active| {
            let mut s = store_h.load();
            s.save_history = active;
            store_h.save(&s);
            glib::Propagation::Proceed
        });

        let weak = Rc::downgrade(self);
        clear_cache_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.image_loader.clear_all();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Thumbnail cache cleared"));
            }
        });

        let weak = Rc::downgrade(self);
        clear_hist_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.history_store.clear();
                w.search_history_store.clear();
                w.toast_overlay
                    .add_toast(adw::Toast::new("Watch & search history cleared"));
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
            let btn = make_filter_item(
                &if sel {
                    format!("{lbl} ✓")
                } else {
                    lbl.to_string()
                },
                sel,
            );
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
                    update_filter_item(
                        b,
                        &if s {
                            format!("{il} ✓")
                        } else {
                            il.to_string()
                        },
                        s,
                    );
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
            let btn = make_filter_item(
                &if sel {
                    format!("{lbl} ✓")
                } else {
                    lbl.to_string()
                },
                sel,
            );
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
                    update_filter_item(
                        b,
                        &if s {
                            format!("{il} ✓")
                        } else {
                            il.to_string()
                        },
                        s,
                    );
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
            let btn = make_filter_item(
                &if sel {
                    format!("{lbl} ✓")
                } else {
                    lbl.to_string()
                },
                sel,
            );
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
                    update_filter_item(
                        b,
                        &if s {
                            format!("{il} ✓")
                        } else {
                            il.to_string()
                        },
                        s,
                    );
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
                update_filter_item(
                    b,
                    &if s {
                        format!("{il} ✓")
                    } else {
                        il.to_string()
                    },
                    s,
                );
            }
            for (iv, il, b) in dub_r.iter() {
                let s = *iv == DurationFilter::AnyLength;
                update_filter_item(
                    b,
                    &if s {
                        format!("{il} ✓")
                    } else {
                        il.to_string()
                    },
                    s,
                );
            }
            for (iv, il, b) in sb_r.iter() {
                let s = *iv == SearchSortOrder::Relevance;
                update_filter_item(
                    b,
                    &if s {
                        format!("{il} ✓")
                    } else {
                        il.to_string()
                    },
                    s,
                );
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

        let subscription_store = self.subscription_store.clone();
        let is_subscribed: Rc<dyn Fn(&str) -> bool> = Rc::new(move |channel_id| {
            !channel_id.is_empty() && subscription_store.is_subscribed(channel_id)
        });

        let weak = Rc::downgrade(self);
        let on_toggle_subscription: Rc<dyn Fn(VideoCollaborator) -> bool> =
            Rc::new(move |channel| {
                let Some(w) = weak.upgrade() else {
                    return false;
                };
                if channel.id.is_empty() {
                    return false;
                }
                let subscribed = w.subscription_store.is_subscribed(&channel.id);
                if subscribed {
                    w.subscription_store.unsubscribe(&channel.id);
                    w.toast_overlay.add_toast(adw::Toast::new(&format!(
                        "Unsubscribed from {}",
                        if channel.name.is_empty() { "channel" } else { channel.name.as_str() }
                    )));
                } else {
                    w.subscription_store.subscribe(SubscribedChannel {
                        id: channel.id.clone(),
                        name: channel.name.clone(),
                        handle: channel.handle.clone(),
                        avatar_url: channel.avatar_url.clone(),
                    });
                    w.toast_overlay.add_toast(adw::Toast::new(&format!(
                        "Subscribed to {}",
                        if channel.name.is_empty() { "channel" } else { channel.name.as_str() }
                    )));
                }
                if *w.active_view.borrow() == ActiveView::Subscriptions {
                    w.load_subscriptions();
                }
                !subscribed
            });

        let weak = Rc::downgrade(self);
        let on_playlists_changed: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(w) = weak.upgrade() {
                let av = w.active_view.borrow().clone();
                match av {
                    ActiveView::Playlists => w.load_playlists_overview(),
                    ActiveView::SavedPlaylistDetail(pid) => w.open_saved_playlist(&pid),
                    _ => {}
                }
            }
        });

        let weak = Rc::downgrade(self);
        let on_toast: Rc<dyn Fn(String)> = Rc::new(move |msg| {
            if let Some(w) = weak.upgrade() {
                w.toast_overlay.add_toast(adw::Toast::new(&msg));
            }
        });

        CardActions {
            engine: self.engine.clone(),
            runtime: self.runtime.clone(),
            image_loader: self.image_loader.clone(),
            playlist_store: self.playlist_store.clone(),
            default_quality,
            is_watch_later,
            on_play,
            on_add_queue,
            on_toggle_watch_later,
            on_download,
            on_open_channel,
            is_subscribed,
            on_toggle_subscription,
            on_playlists_changed,
            on_toast,
        }
    }

    fn next_playback_seq(&self) -> u64 {
        let next = self.playback_seq.get().wrapping_add(1);
        self.playback_seq.set(next);
        next
    }

    fn next_mpv_sync_gen(&self) -> u64 {
        let next = self.mpv_sync_gen.get().wrapping_add(1).max(1);
        self.mpv_sync_gen.set(next);
        next
    }

    fn reset_mpv_queue_tracking(&self) {
        self.next_playback_seq();
        self.mpv_sync_gen.set(0);
        self.mpv_acked_gen.set(0);
        *self.queue_continuation.borrow_mut() = None;
        self.queue_cont_fetching.set(false);
    }

    fn remote_playlist_queue_continuation(&self) -> Option<QueueContinuation> {
        if matches!(*self.active_view.borrow(), ActiveView::RemotePlaylist(_)) {
            self.feed_cursor.borrow().clone().map(|cursor| QueueContinuation {
                cursor,
                fallback_details: None,
                is_shorts: false,
            })
        } else {
            None
        }
    }

    pub fn play_video(self: &Rc<Self>, summary: VideoSummary, quality: Quality) {
        self.play_queue_from(vec![summary], 0, quality, false);
    }

    pub fn play_shorts_queue(
        self: &Rc<Self>,
        shorts: Vec<VideoSummary>,
        start_index: usize,
        explicit_quality: Option<Quality>,
        continuation: Option<(PageCursor, Option<ChannelDetails>)>,
    ) {
        if shorts.is_empty() {
            return;
        }
        let mut quality = explicit_quality.unwrap_or_else(|| {
            let settings = self.settings_store.load();
            Quality::from_setting(&settings.default_quality)
        });
        if quality == Quality::AudioOnly {
            quality = Quality::P360;
        }
        let cont = continuation.map(|(cursor, fallback_details)| QueueContinuation {
            cursor,
            fallback_details,
            is_shorts: true,
        });
        self.play_queue_from_with_continuation(shorts, start_index, quality, true, cont);
    }

    pub fn play_queue_from(
        self: &Rc<Self>,
        items: Vec<VideoSummary>,
        start_index: usize,
        quality: Quality,
        shorts_mode: bool,
    ) {
        self.play_queue_from_with_continuation(items, start_index, quality, shorts_mode, None);
    }

    pub fn play_queue_from_with_continuation(
        self: &Rc<Self>,
        items: Vec<VideoSummary>,
        start_index: usize,
        quality: Quality,
        shorts_mode: bool,
        continuation: Option<QueueContinuation>,
    ) {
        if items.is_empty() {
            return;
        }
        let clamped_start = start_index.min(items.len() - 1);
        self.playback_quality.set(quality);
        *self.queue_continuation.borrow_mut() = continuation;
        self.queue_cont_fetching.set(false);
        self.player_card.set_queue(items, clamped_start, shorts_mode);
        self.start_mpv_full_queue();
    }

    fn to_mpv_queue_items(items: &[VideoSummary]) -> Vec<MpvQueueItem> {
        items
            .iter()
            .map(|v| MpvQueueItem {
                video_id: v.id.clone(),
                title: v.title.clone(),
            })
            .collect()
    }

    fn jump_or_reload_queue_index(self: &Rc<Self>, target_idx: usize) {
        if self.mpv_sync_gen.get() > 0 && target_idx < self.player_card.queue_len() {
            let session_opt = self.mpv_session.borrow().clone();
            if let Some(session) = session_opt {
                if session.is_alive() {
                    self.mpv_pos.set(0.0);
                    if let Some(cur) = self.player_card.current_video() {
                        self.mpv_duration
                            .set(cur.duration_seconds.unwrap_or(0) as f64);
                        if self.settings_store.load().save_history {
                            self.history_store.add(cur);
                        }
                    }
                    self.player_card.set_playing(true);
                    self.player_card.set_loading(true);
                    session.set_playlist_pos(target_idx);
                    self.prefetch_next_track_pool();
                    self.pump_queue_continuation();
                    return;
                }
            }
        }
        self.start_mpv_full_queue();
    }

    fn resync_mpv_upcoming_queue(self: &Rc<Self>) {
        if self.mpv_sync_gen.get() == 0 || self.player_card.is_empty() {
            return;
        }
        let Some(session) = self.mpv_session.borrow().clone() else {
            return;
        };
        if !session.is_alive() {
            return;
        }

        let all = self.player_card.all_items();
        let cur_idx = self.player_card.current_idx();
        let sync_gen = self.next_mpv_sync_gen();
        session.sync_queue(&Self::to_mpv_queue_items(&all), cur_idx, sync_gen);
        self.prefetch_next_track_pool();
        self.pump_queue_continuation();
    }

    fn start_mpv_full_queue(self: &Rc<Self>) {
        let all = self.player_card.all_items();
        if all.is_empty() {
            return;
        }
        let cur_idx = self.player_card.current_idx().min(all.len() - 1);
        let _seq = self.next_playback_seq();
        let sync_gen = self.next_mpv_sync_gen();

        let quality = self.playback_quality.get();
        let session = self.ensure_mpv_session();
        session.set_default_quality(quality);

        self.mpv_became_active.set(false);
        self.mpv_pos.set(0.0);
        if let Some(cur) = all.get(cur_idx) {
            self.mpv_duration
                .set(cur.duration_seconds.unwrap_or(0) as f64);
            if self.settings_store.load().save_history {
                self.history_store.add(cur.clone());
            }
            // Resolve stream pool in parallel while MPV initializes its IPC socket
            let eng = self.engine.clone();
            let cur_id = cur.id.clone();
            self.runtime.spawn(async move {
                eng.prefetch_video_pool(&cur_id).await;
            });
        }

        self.player_card.set_playing(true);
        self.player_card.set_loading(true);

        session.load_queue(
            &Self::to_mpv_queue_items(&all),
            cur_idx,
            quality == Quality::AudioOnly,
            sync_gen,
        );
        self.prefetch_next_track_pool();
        self.pump_queue_continuation();
    }

    fn prefetch_next_track_pool(&self) {
        let next_idx = self.player_card.current_idx() + 1;
        if let Some(next_item) = self.player_card.item_at(next_idx) {
            let engine = self.engine.clone();
            let vid_id = next_item.id;
            self.runtime.spawn(async move {
                engine.prefetch_video_pool(&vid_id).await;
            });
        }
    }

    fn pump_queue_continuation(self: &Rc<Self>) {
        const REFILL_AHEAD_THRESHOLD: usize = 20;
        const ROLLING_AHEAD_TARGET: usize = 48;

        if self.queue_cont_fetching.get() || self.player_card.is_empty() {
            return;
        }
        let cur_idx = self.player_card.current_idx();
        let q_len = self.player_card.queue_len();
        let remaining_ahead = q_len.saturating_sub(cur_idx + 1);
        if remaining_ahead >= REFILL_AHEAD_THRESHOLD {
            return;
        }
        self.pump_queue_continuation_until(ROLLING_AHEAD_TARGET);
    }

    fn pump_queue_continuation_until(self: &Rc<Self>, target_ahead: usize) {
        if self.queue_cont_fetching.get() || self.player_card.is_empty() {
            return;
        }
        let Some(cont) = self.queue_continuation.borrow().clone() else {
            return;
        };

        let cur_idx = self.player_card.current_idx();
        let q_len = self.player_card.queue_len();
        let remaining_ahead = q_len.saturating_sub(cur_idx + 1);
        if remaining_ahead >= target_ahead {
            return;
        }

        self.queue_cont_fetching.set(true);
        let seq = self.playback_seq.get();
        let fallback_for_next = cont.fallback_details.clone();
        let is_shorts_for_next = cont.is_shorts;
        let engine = self.engine.clone();
        let (tx, rx) = async_channel::bounded::<Result<PagedVideos, String>>(1);

        self.runtime.spawn(async move {
            let res = engine
                .load_more(
                    &cont.cursor,
                    cont.fallback_details.as_ref(),
                    cont.is_shorts,
                    false,
                )
                .await;
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
            if w.playback_seq.get() != seq {
                return;
            }
            w.queue_cont_fetching.set(false);

            match res {
                Ok(paged) => {
                    let has_items = !paged.items.is_empty();
                    let next_cont = if has_items {
                        paged.next.map(|cursor| QueueContinuation {
                            cursor,
                            fallback_details: fallback_for_next,
                            is_shorts: is_shorts_for_next,
                        })
                    } else {
                        None
                    };
                    *w.queue_continuation.borrow_mut() = next_cont;
                    if has_items {
                        let added = w.player_card.append_items(paged.items);
                        if !added.is_empty() && w.mpv_sync_gen.get() > 0 {
                            if let Some(session) = w.mpv_session.borrow().as_ref() {
                                if session.is_alive() {
                                    session.append_queue(&Self::to_mpv_queue_items(&added));
                                }
                            }
                        }
                        w.prefetch_next_track_pool();
                        w.pump_queue_continuation_until(target_ahead);
                    }
                }
                Err(_) => {
                    *w.queue_continuation.borrow_mut() = None;
                }
            }
        });
    }

    pub fn queue_video(self: &Rc<Self>, summary: VideoSummary) {
        if self.player_card.is_empty() {
            let def_q = Quality::from_setting(&self.settings_store.load().default_quality);
            self.play_video(summary, def_q);
            return;
        }

        let mpv_item = MpvQueueItem {
            video_id: summary.id.clone(),
            title: summary.title.clone(),
        };
        self.player_card.add_item(summary);
        if self.mpv_sync_gen.get() > 0 {
            if let Some(session) = self.mpv_session.borrow().as_ref() {
                if session.is_alive() {
                    session.append_queue(&[mpv_item]);
                }
            }
        }
        self.prefetch_next_track_pool();
        self.toast_overlay
            .add_toast(adw::Toast::new("Added to queue"));
    }

    pub fn start_download(self: &Rc<Self>, summary: VideoSummary, quality: Quality) {
        let downloads_dir = glib::user_special_dir(glib::UserDirectory::Downloads)
            .unwrap_or_else(|| glib::home_dir().join("Downloads"));

        let short_title = summary.title.clone();
        self.toast_overlay.add_toast(adw::Toast::new(&format!(
            "Downloading “{short_title}” to Downloads…"
        )));

        let vid_id = summary.id.clone();
        let title_for_dl = summary.title.clone();
        let engine = self.engine.clone();
        let (tx, rx) = async_channel::bounded::<Result<std::path::PathBuf, String>>(1);
        self.runtime.spawn(async move {
            let res = engine
                .download_video(&vid_id, &title_for_dl, quality, downloads_dir)
                .await;
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
                        "Saved “{short_title}” to Downloads"
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
        let session = MpvSession::spawn(&self.runtime, self.engine.clone(), &settings, sender);
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
                        if dur > 0.0 {
                            w.mpv_duration.set(dur);
                            let pos = w.mpv_pos.get();
                            w.player_card.set_progress(pos, dur);
                        }
                    }
                    MpvEvent::Pause(p) => {
                        w.player_card.set_playing(!p);
                    }
                    MpvEvent::QueueSynced(gen_num) => {
                        if gen_num > w.mpv_acked_gen.get() {
                            w.mpv_acked_gen.set(gen_num);
                        }
                    }
                    MpvEvent::PlaylistPos(mpv_idx) => {
                        let sync_gen = w.mpv_sync_gen.get();
                        if sync_gen > 0
                            && w.mpv_acked_gen.get() == sync_gen
                            && mpv_idx < w.player_card.queue_len()
                            && mpv_idx != w.player_card.current_idx()
                        {
                            w.player_card.set_current_index(mpv_idx);
                            w.mpv_pos.set(0.0);
                            if let Some(cur) = w.player_card.current_video() {
                                w.mpv_duration
                                    .set(cur.duration_seconds.unwrap_or(0) as f64);
                                if w.settings_store.load().save_history {
                                    w.history_store.add(cur);
                                }
                            }
                            w.player_card.set_playing(true);
                            w.prefetch_next_track_pool();
                            w.pump_queue_continuation();
                        }
                    }
                    MpvEvent::TrackEnded => {
                        let cur_idx = w.player_card.current_idx();
                        if cur_idx + 1 >= w.player_card.queue_len()
                            && w.queue_continuation.borrow().is_none()
                        {
                            w.player_card.set_loading(false);
                            w.player_card.set_playing(false);
                        }
                    }
                    MpvEvent::Idle(idle) => {
                        if !idle {
                            w.mpv_became_active.set(true);
                        } else if w.mpv_became_active.get() {
                            w.player_card.set_loading(false);
                        }
                    }
                    MpvEvent::Error(msg) => {
                        w.player_card.set_loading(false);
                        w.toast_overlay.add_toast(adw::Toast::new(&msg));
                    }
                    MpvEvent::ProcessExited => {
                        *w.mpv_session.borrow_mut() = None;
                        w.reset_mpv_queue_tracking();
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

    fn bind_home_picture(&self, key: &str, url: &str, picture: &gtk4::Picture) {
        if url.is_empty() {
            return;
        }
        self.home_pictures.borrow_mut().push((
            key.to_string(),
            url.to_string(),
            false,
            picture.downgrade(),
        ));
        if !self.home_textures_suspended.get() {
            self.image_loader.load_into_picture(key, url, picture);
        }
    }

    fn bind_home_short_picture(&self, key: &str, url: &str, picture: &gtk4::Picture) {
        if url.is_empty() {
            return;
        }
        self.home_pictures.borrow_mut().push((
            key.to_string(),
            url.to_string(),
            true,
            picture.downgrade(),
        ));
        if !self.home_textures_suspended.get() {
            self.image_loader.load_into_short_picture(key, url, picture);
        }
    }

    fn bind_home_avatar(&self, key: &str, url: &str, avatar: &adw::Avatar) {
        if url.is_empty() {
            return;
        }
        self.home_avatars
            .borrow_mut()
            .push((key.to_string(), url.to_string(), avatar.downgrade()));
        if !self.home_textures_suspended.get() {
            self.image_loader.load_into_avatar(key, url, avatar);
        }
    }

    fn clear_home_flow_box(&self) {
        self.home_pictures.borrow_mut().clear();
        self.home_avatars.borrow_mut().clear();
        self.last_home_cull_y.set(0.0);
        let had_children = self.flow_box.first_child().is_some();
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        if had_children {
            self.image_loader.schedule_heap_trim();
        }
    }

    fn suspend_home_textures(&self) {
        if self.home_textures_suspended.replace(true) {
            return;
        }
        for (_, _, _, wref) in self.home_pictures.borrow().iter() {
            if let Some(pic) = wref.upgrade() {
                pic.set_paintable(gdk::Paintable::NONE);
            }
        }
        for (_, _, wref) in self.home_avatars.borrow().iter() {
            if let Some(av) = wref.upgrade() {
                av.set_custom_image(gdk::Paintable::NONE);
            }
        }
        self.image_loader.shrink_textures(8);
        self.image_loader.schedule_heap_trim();
    }

    fn resume_home_textures(&self) {
        if !self.home_textures_suspended.replace(false) {
            return;
        }
        let view_h = self.content_scrolled.height();
        let can_cull = view_h > 0 && self.home_pictures.borrow().len() > 28;
        let max_y = view_h as f32 + 950.0;

        let pics: Vec<(String, String, bool, glib::WeakRef<gtk4::Picture>)> = self
            .home_pictures
            .borrow()
            .iter()
            .filter(|(_, _, _, w)| w.upgrade().is_some())
            .cloned()
            .collect();
        *self.home_pictures.borrow_mut() = pics.clone();

        for (key, url, is_short_pic, wref) in &pics {
            if let Some(pic) = wref.upgrade() {
                let should_load = if can_cull {
                    pic.compute_bounds(&self.content_scrolled)
                        .map(|b| b.y() + b.height() >= -950.0 && b.y() <= max_y)
                        .unwrap_or(true)
                } else {
                    true
                };
                if should_load && pic.paintable().is_none() {
                    if *is_short_pic {
                        self.image_loader.load_into_short_picture(key, url, &pic);
                    } else {
                        self.image_loader.load_into_picture(key, url, &pic);
                    }
                }
            }
        }

        let avs: Vec<(String, String, glib::WeakRef<adw::Avatar>)> = self
            .home_avatars
            .borrow()
            .iter()
            .filter(|(_, _, w)| w.upgrade().is_some())
            .cloned()
            .collect();
        *self.home_avatars.borrow_mut() = avs.clone();

        for (key, url, wref) in &avs {
            if let Some(av) = wref.upgrade() {
                let should_load = if can_cull {
                    av.compute_bounds(&self.content_scrolled)
                        .map(|b| b.y() + b.height() >= -950.0 && b.y() <= max_y)
                        .unwrap_or(true)
                } else {
                    true
                };
                if should_load && av.custom_image().is_none() {
                    self.image_loader.load_into_avatar(key, url, &av);
                }
            }
        }
    }

    fn maybe_cull_home_textures(&self) {
        if self.home_textures_suspended.get() {
            return;
        }
        let is_home = self
            .nav_view
            .visible_page()
            .and_then(|p| p.tag())
            .map(|t| t.as_str() == "home")
            .unwrap_or(true);
        if !is_home {
            return;
        }
        let pic_count = self.home_pictures.borrow().len();
        if pic_count <= 28 {
            return;
        }
        let view_h = self.content_scrolled.height();
        if view_h <= 0 {
            return;
        }

        let scroll_y = self.content_scrolled.vadjustment().value();
        if scroll_y > 10.0 && (scroll_y - self.last_home_cull_y.get()).abs() < 260.0 {
            return;
        }
        self.last_home_cull_y.set(scroll_y);

        let min_y: f32 = -950.0;
        let max_y: f32 = view_h as f32 + 950.0;
        let mut culled_any = false;

        let pics: Vec<(String, String, bool, glib::WeakRef<gtk4::Picture>)> =
            self.home_pictures.borrow().clone();
        for (key, url, is_short_pic, wref) in &pics {
            let Some(pic) = wref.upgrade() else {
                continue;
            };
            let Some(bounds) = pic.compute_bounds(&self.content_scrolled) else {
                continue;
            };
            let in_band = bounds.y() + bounds.height() >= min_y && bounds.y() <= max_y;
            if in_band {
                if pic.paintable().is_none() {
                    if *is_short_pic {
                        self.image_loader.load_into_short_picture(key, url, &pic);
                    } else {
                        self.image_loader.load_into_picture(key, url, &pic);
                    }
                }
            } else if pic.paintable().is_some() {
                pic.set_paintable(gdk::Paintable::NONE);
                culled_any = true;
            }
        }

        let avs: Vec<(String, String, glib::WeakRef<adw::Avatar>)> =
            self.home_avatars.borrow().clone();
        for (key, url, wref) in &avs {
            let Some(av) = wref.upgrade() else {
                continue;
            };
            let Some(bounds) = av.compute_bounds(&self.content_scrolled) else {
                continue;
            };
            let in_band = bounds.y() + bounds.height() >= min_y && bounds.y() <= max_y;
            if in_band {
                if av.custom_image().is_none() {
                    self.image_loader.load_into_avatar(key, url, &av);
                }
            } else if av.custom_image().is_some() {
                av.set_custom_image(gdk::Paintable::NONE);
                culled_any = true;
            }
        }

        if culled_any {
            self.image_loader.schedule_heap_trim();
        }
    }

    pub fn open_channel(
        self: &Rc<Self>,
        channel_id: &str,
        channel_name: &str,
    ) -> Option<Rc<ChannelPage>> {
        if channel_id.is_empty() {
            return None;
        }
        self.defocus_search_bar();
        self.suspend_home_textures();
        for existing_cp in self.open_channels.borrow().iter() {
            existing_cp.suspend_textures();
        }

        let actions = self.make_card_actions();
        let weak_shorts = Rc::downgrade(self);
        let on_play_shorts: ShortsPlayHandler =
            Rc::new(move |shorts, start_idx, explicit_q, cont| {
                if let Some(w) = weak_shorts.upgrade() {
                    w.play_shorts_queue(shorts, start_idx, explicit_q, cont);
                }
            });

        let weak_pl = Rc::downgrade(self);
        let on_open_playlist: Rc<dyn Fn(String)> = Rc::new(move |playlist_id| {
            if let Some(w) = weak_pl.upgrade() {
                w.open_remote_playlist(&playlist_id);
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
            on_open_playlist,
            self.is_narrow.get(),
            self.is_compact.get(),
            self.grid_cols.get(),
            self.shorts_cols.get(),
        );

        self.open_channels.borrow_mut().push(channel_page.clone());
        self.nav_view.push(&channel_page.page);
        self.defocus_search_bar();
        channel_page.reload();

        Some(channel_page)
    }

    pub fn reload_current_context(self: &Rc<Self>) {
        if let Some(vis_tag) = self
            .nav_view
            .visible_page()
            .and_then(|p| p.tag())
            .map(|g| g.to_string())
        {
            if vis_tag != "home" {
                let cp_opt = self
                    .open_channels
                    .borrow()
                    .iter()
                    .rev()
                    .find(|cp| cp.channel_id() == vis_tag)
                    .cloned();
                if let Some(cp) = cp_opt {
                    cp.reload();
                    return;
                }
            }
        }

        let av = self.active_view.borrow().clone();
        let this = self.clone();
        self.with_suppressed_nav_push(|| match av {
            ActiveView::HomeTrending => this.load_trending_opts(true),
            ActiveView::HomeCategory(cat) => this.search_category_opts(&cat, true),
            ActiveView::WatchLater => this.load_watch_later(),
            ActiveView::Search(query) => this.search(&query),
            ActiveView::Subscriptions => this.load_subscriptions(),
            ActiveView::Playlists => this.load_playlists_overview(),
            ActiveView::SavedPlaylistDetail(pid) => this.open_saved_playlist(&pid),
            ActiveView::RemotePlaylist(pid) => this.open_remote_playlist(&pid),
            ActiveView::History => this.load_history(),
            ActiveView::Preferences => {
                let current = this.settings_store.load();
                this.sync_preferences_ui(&current);
            }
        });
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
        self.defocus_search_bar();
        let was_on_subpage = self
            .nav_view
            .visible_page()
            .and_then(|p| p.tag())
            .map(|t| t.as_str() != "home")
            .unwrap_or(false);
        let popped_channel_info = if was_on_subpage {
            self.open_channels
                .borrow()
                .last()
                .map(|cp| (cp.channel_id().to_string(), cp.page.title().to_string()))
        } else {
            None
        };

        self.home_pictures.borrow_mut().clear();
        self.home_avatars.borrow_mut().clear();
        self.home_textures_suspended.set(false);
        self.nav_view.pop_to_page(&self.home_page);
        self.clear_card_selection();

        if !self.suppress_nav_push.get() {
            let old_view = self.active_view.borrow().clone();
            if old_view != view {
                let mut hist = self.nav_history.borrow_mut();
                hist.push(NavHistoryEntry::MainView {
                    view: old_view,
                    search_tab: self.search_tab.get(),
                    search_date: self.search_date_filter.get(),
                    search_dur: self.search_dur_filter.get(),
                    search_sort: self.search_sort_order.get(),
                });
                if let Some((cid, cname)) = popped_channel_info {
                    hist.push(NavHistoryEntry::Channel {
                        id: cid,
                        name: cname,
                    });
                }
                if hist.len() > 32 {
                    let drop_n = hist.len() - 32;
                    hist.drain(0..drop_n);
                }
            } else if let Some((cid, cname)) = popped_channel_info {
                let mut hist = self.nav_history.borrow_mut();
                hist.push(NavHistoryEntry::Channel {
                    id: cid,
                    name: cname,
                });
                if hist.len() > 32 {
                    let drop_n = hist.len() - 32;
                    hist.drain(0..drop_n);
                }
            }
        }

        *self.active_view.borrow_mut() = view.clone();
        *self.feed_cursor.borrow_mut() = None;
        self.trending_cursors.borrow_mut().clear();
        self.is_loading_more.set(false);
        self.view_mode.set(mode);

        let is_prefs = view == ActiveView::Preferences;
        self.feed_box.set_visible(!is_prefs);
        self.prefs_clamp.set_visible(is_prefs);

        self.content_scrolled.vadjustment().set_value(0.0);

        if is_prefs {
            self.update_back_button_visibility();
            return;
        }

        let is_pl_detail = matches!(
            view,
            ActiveView::SavedPlaylistDetail(_) | ActiveView::RemotePlaylist(_)
        );

        self.chips_scroll.set_visible(show_chips);
        self.subs_strip_box.set_visible(show_subs);
        self.history_header_box.set_visible(show_history_header);
        self.playlist_header_box.set_visible(is_pl_detail);

        let is_search = matches!(view, ActiveView::Search(_));
        let is_wl = view == ActiveView::WatchLater;
        let is_playlists_home = view == ActiveView::Playlists;

        self.search_filters_btn.set_visible(is_search);
        self.search_tabs_scroll.set_visible(is_search);
        self.clear_watch_later_btn.set_visible(is_wl);
        self.new_playlist_btn.set_visible(is_playlists_home);

        if is_playlists_home {
            self.section_title.remove_css_class("yr-h2");
            self.section_title.remove_css_class("yr-search-heading");
            self.section_title.add_css_class("yr-h1");
        } else if is_search {
            self.section_title.remove_css_class("yr-h1");
            self.section_title.remove_css_class("yr-h2");
            self.section_title.add_css_class("yr-search-heading");
        } else {
            self.section_title.remove_css_class("yr-h1");
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

    pub fn submit_search_or_url(self: &Rc<Self>, raw_input: &str) {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() {
            return;
        }
        if looks_like_youtube_url(trimmed) {
            self.open_pasted_youtube_url(trimmed);
            return;
        }
        if self.settings_store.load().save_history {
            self.search_history_store.add(trimmed);
        }
        self.search(trimmed);
    }

    fn open_pasted_youtube_url(self: &Rc<Self>, url: &str) {
        self.toast_overlay
            .add_toast(adw::Toast::new("Opening YouTube link…"));

        let engine = self.engine.clone();
        let url_owned = url.to_string();
        let (tx, rx) = async_channel::bounded::<Result<ResolvedUrl, String>>(1);
        self.runtime.spawn(async move {
            let res = engine.resolve_youtube_url(&url_owned).await;
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
                Ok(ResolvedUrl::Video(summary)) => {
                    w.clear_search_bar_quietly();
                    let def_q = Quality::from_setting(&w.settings_store.load().default_quality);
                    if summary.is_short {
                        w.play_shorts_queue(vec![summary], 0, Some(def_q), None);
                    } else {
                        w.play_video(summary, def_q);
                    }
                }
                Ok(ResolvedUrl::Channel { id, name }) => {
                    w.clear_search_bar_quietly();
                    w.open_channel(&id, &name);
                }
                Ok(ResolvedUrl::Playlist { id }) => {
                    w.clear_search_bar_quietly();
                    w.open_remote_playlist(&id);
                }
                Err(err) => {
                    w.toast_overlay.add_toast(adw::Toast::new(&err));
                }
            }
        });
    }

    pub fn search(self: &Rc<Self>, query: &str) {
        let req_id = self.next_request_id();
        self.set_active_nav(None);
        let tab = self.search_tab.get();
        self.highlight_search_tab(tab);
        let search_view_mode = if tab == SearchTab::Shorts {
            ViewMode::ShortsGrid
        } else {
            ViewMode::SearchList
        };

        self.prepare_view_shell(
            ActiveView::Search(query.to_string()),
            search_view_mode,
            false,
            false,
            false,
            Some(&format!("Results for “{query}”")),
        );
        if tab == SearchTab::Shorts {
            self.show_skeletons_shorts(12);
        } else {
            self.show_skeletons_search(6);
        }

        let (sender, receiver) = async_channel::bounded::<Result<PagedSearchItems, String>>(1);
        let engine = self.engine.clone();
        let q = query.to_string();
        let q_ui = query.to_string();
        let df = self.search_date_filter.get();
        let duf = self.search_dur_filter.get();
        let sf = self.search_sort_order.get();

        self.runtime.spawn(async move {
            let res = engine.search_items(&q, tab, df, duf, sf).await;
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
                                "Nothing matched “{q_ui}”. Try another tab or different keywords."
                            ),
                            Some((
                                "Clear search",
                                Rc::new(move || {
                                    if let Some(win) = weak_clear.upgrade() {
                                        if win.nav_history.borrow().is_empty() {
                                            let w2 = win.clone();
                                            win.reset_nav_root(|| {
                                                w2.clear_search_bar_quietly();
                                                w2.set_active_nav(Some(RailTab::Home));
                                                w2.highlight_chip(Some(&w2.chip_trending));
                                                w2.load_trending();
                                            });
                                        } else {
                                            win.go_back();
                                        }
                                    }
                                }),
                            )),
                        );
                    } else {
                        *w.feed_cursor.borrow_mut() = paged.next;
                        w.populate_search_items(paged.items);
                    }
                }
                Err(e) => {
                    let weak_retry = Rc::downgrade(&w);
                    let q_retry = q_ui.clone();
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Could not load search results",
                        &e,
                        Some((
                            "Retry",
                            Rc::new(move || {
                                if let Some(win) = weak_retry.upgrade() {
                                    win.search(&q_retry);
                                }
                            }),
                        )),
                    );
                }
            }
        });
    }

    fn populate_search_items(self: &Rc<Self>, items: Vec<SearchItem>) {
        self.clear_card_selection();
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        self.clear_home_flow_box();
        let mode = if self.search_tab.get() == SearchTab::Shorts {
            ViewMode::ShortsGrid
        } else {
            ViewMode::SearchList
        };
        self.view_mode.set(mode);
        self.update_flow_columns();
        *self.current_search_items.borrow_mut() = Vec::new();
        *self.current_videos.borrow_mut() = Vec::new();
        self.append_search_items(items);
    }

    fn append_search_items(self: &Rc<Self>, items: Vec<SearchItem>) {
        let mut actions = self.make_card_actions();
        let weak_play = Rc::downgrade(self);
        actions.on_play = Rc::new(move |sum, q| {
            let Some(w) = weak_play.upgrade() else {
                return;
            };
            let is_shorts_tab = w.search_tab.get() == SearchTab::Shorts;
            if sum.is_short || is_shorts_tab {
                let mut shorts: Vec<VideoSummary> = w
                    .current_search_items
                    .borrow()
                    .iter()
                    .filter_map(|it| match it {
                        SearchItem::Video(v) if v.is_short || is_shorts_tab => Some(v.clone()),
                        _ => None,
                    })
                    .collect();
                let idx = shorts
                    .iter()
                    .position(|v| v.id == sum.id)
                    .unwrap_or_else(|| {
                        shorts.insert(0, sum);
                        0
                    });
                let cont = if is_shorts_tab {
                    w.feed_cursor.borrow().clone().map(|c| (c, None))
                } else {
                    None
                };
                w.play_shorts_queue(shorts, idx, Some(q), cont);
            } else {
                w.play_video(sum, q);
            }
        });

        let mode = if self.search_tab.get() == SearchTab::Shorts {
            ViewMode::ShortsGrid
        } else {
            ViewMode::SearchList
        };
        let layout = self.list_card_layout();

        for item in items {
            match &item {
                SearchItem::Video(summary) => {
                    self.append_single_video_card(summary, mode, layout, &actions, None, true);
                    self.current_videos.borrow_mut().push(summary.clone());
                }
                SearchItem::Channel(ch) => {
                    let initially_sub = self.subscription_store.is_subscribed(&ch.id);
                    let weak_open = Rc::downgrade(self);
                    let cid_open = ch.id.clone();
                    let cname_open = ch.name.clone();
                    let open_cb: Rc<dyn Fn()> = Rc::new(move || {
                        if let Some(w) = weak_open.upgrade() {
                            w.open_channel(&cid_open, &cname_open);
                        }
                    });

                    let weak_sel = Rc::downgrade(self);
                    let open_enter = open_cb.clone();
                    let open_dbl = open_cb;

                    let weak_sub = Rc::downgrade(self);
                    let ch_sub = ch.clone();

                    let card = ChannelResultCard::new(
                        ch,
                        layout,
                        initially_sub,
                        move |hl| {
                            if let Some(w) = weak_sel.upgrade() {
                                w.select_card(hl, open_enter.clone());
                            }
                        },
                        move || open_dbl(),
                        move || {
                            let Some(w) = weak_sub.upgrade() else {
                                return false;
                            };
                            if w.subscription_store.is_subscribed(&ch_sub.id) {
                                w.subscription_store.unsubscribe(&ch_sub.id);
                                w.toast_overlay.add_toast(adw::Toast::new(&format!(
                                    "Unsubscribed from {}",
                                    ch_sub.name
                                )));
                                false
                            } else {
                                w.subscription_store.subscribe(SubscribedChannel {
                                    id: ch_sub.id.clone(),
                                    name: ch_sub.name.clone(),
                                    handle: ch_sub.handle.clone(),
                                    avatar_url: ch_sub.avatar_url.clone(),
                                });
                                w.toast_overlay.add_toast(adw::Toast::new(&format!(
                                    "Subscribed to {}",
                                    ch_sub.name
                                )));
                                true
                            }
                        },
                    );

                    if !ch.avatar_url.is_empty() {
                        self.bind_home_avatar(&ch.id, &ch.avatar_url, &card.avatar);
                    }
                    self.flow_box.append(&card.container);
                }
                SearchItem::Playlist(pl) => {
                    let initially_saved = self
                        .playlist_store
                        .find_by_youtube_id(&pl.id)
                        .is_some();

                    let weak_open = Rc::downgrade(self);
                    let pid_open = pl.id.clone();
                    let open_cb: Rc<dyn Fn()> = Rc::new(move || {
                        if let Some(w) = weak_open.upgrade() {
                            w.open_remote_playlist(&pid_open);
                        }
                    });

                    let weak_sel = Rc::downgrade(self);
                    let open_enter = open_cb.clone();
                    let open_dbl = open_cb;

                    let weak_save = Rc::downgrade(self);
                    let pl_save = pl.clone();

                    let card = PlaylistResultCard::new(
                        pl,
                        layout,
                        initially_saved,
                        move |hl| {
                            if let Some(w) = weak_sel.upgrade() {
                                w.select_card(hl, open_enter.clone());
                            }
                        },
                        move || open_dbl(),
                        move || {
                            if let Some(w) = weak_save.upgrade() {
                                w.save_youtube_playlist_in_background(&pl_save);
                            }
                        },
                    );

                    if !pl.thumbnail_url.is_empty() {
                        self.bind_home_picture(
                            &format!("pl_{}", pl.id),
                            &pl.thumbnail_url,
                            &card.picture,
                        );
                    }
                    self.flow_box.append(&card.container);
                }
            }
            self.current_search_items.borrow_mut().push(item);
        }
    }

    fn save_youtube_playlist_in_background(self: &Rc<Self>, pl_meta: &PlaylistSearchSummary) {
        let engine = self.engine.clone();
        let pid = pl_meta.id.clone();
        let fallback_name = pl_meta.name.clone();
        let (tx, rx) = async_channel::bounded::<Result<RemotePlaylist, String>>(1);
        self.runtime.spawn(async move {
            let res = engine.fetch_playlist(&pid).await;
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
                Ok(remote) => {
                    let saved = w.playlist_store.create_with_videos(
                        &remote.name,
                        remote.videos,
                        Some(remote.id),
                        &remote.channel_name,
                    );
                    w.toast_overlay
                        .add_toast(adw::Toast::new(&format!("Saved playlist “{}”", saved.name)));
                }
                Err(err) => {
                    w.toast_overlay.add_toast(adw::Toast::new(&format!(
                        "Could not save “{fallback_name}”: {err}"
                    )));
                }
            }
        });
    }

    pub fn load_playlists_overview(self: &Rc<Self>) {
        let _req_id = self.next_request_id();
        let playlists = self.playlist_store.load_all();

        self.prepare_view_shell(
            ActiveView::Playlists,
            ViewMode::Grid,
            false,
            false,
            false,
            Some("Playlists"),
        );

        self.clear_home_flow_box();

        if playlists.is_empty() {
            self.show_status_state(
                "folder-music-symbolic",
                "No playlists yet",
                "Click “+ New playlist” above, save your playing queue, or right-click any video and choose Playlist +.",
                None,
            );
            return;
        }

        for pl in playlists {
            let pid_open = pl.id.clone();
            let weak_open = Rc::downgrade(self);
            let open_cb: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(w) = weak_open.upgrade() {
                    w.open_saved_playlist(&pid_open);
                }
            });

            let weak_sel = Rc::downgrade(self);
            let open_enter = open_cb.clone();
            let open_dbl = open_cb;

            let weak_play = Rc::downgrade(self);
            let vids_play = pl.videos.clone();

            let weak_del = Rc::downgrade(self);
            let pid_del = pl.id.clone();
            let pname_del = pl.name.clone();

            let card = SavedPlaylistCard::new(
                &pl,
                move |hl| {
                    if let Some(w) = weak_sel.upgrade() {
                        w.select_card(hl, open_enter.clone());
                    }
                },
                move || open_dbl(),
                move || {
                    if let Some(w) = weak_play.upgrade() {
                        if !vids_play.is_empty() {
                            let q =
                                Quality::from_setting(&w.settings_store.load().default_quality);
                            w.play_queue_from(vids_play.clone(), 0, q, false);
                        }
                    }
                },
                move || {
                    if let Some(w) = weak_del.upgrade() {
                        w.playlist_store.delete(&pid_del);
                        w.toast_overlay
                            .add_toast(adw::Toast::new(&format!("Deleted “{pname_del}”")));
                        w.load_playlists_overview();
                    }
                },
            );

            if let Some(first_vid) = pl.videos.first() {
                if !first_vid.thumbnail_url.is_empty() {
                    self.bind_home_picture(
                        &first_vid.id,
                        &first_vid.thumbnail_url,
                        &card.picture,
                    );
                }
            }

            self.flow_box.append(&card.container);
        }
    }

    pub fn open_saved_playlist(self: &Rc<Self>, playlist_id: &str) {
        let _req_id = self.next_request_id();
        let Some(pl) = self.playlist_store.get(playlist_id) else {
            self.load_playlists_overview();
            return;
        };

        self.prepare_view_shell(
            ActiveView::SavedPlaylistDetail(pl.id.clone()),
            ViewMode::SearchList,
            false,
            false,
            false,
            None,
        );

        self.playlist_title_lbl.set_text(&pl.name);
        let total_secs: u64 = pl.videos.iter().filter_map(|v| v.duration_seconds).sum();
        let dur_str = format_queue_total_duration(total_secs);
        let count = pl.videos.len();
        let word = if count == 1 { "video" } else { "videos" };
        let meta = if dur_str.is_empty() {
            format!("{count} {word}")
        } else {
            format!("{count} {word} · {dur_str}")
        };
        self.playlist_meta_lbl.set_text(&meta);

        self.playlist_save_remote_btn.set_visible(false);
        self.playlist_rename_btn.set_visible(true);
        self.playlist_delete_btn.set_visible(true);

        self.clear_home_flow_box();

        *self.current_videos.borrow_mut() = pl.videos.clone();

        if pl.videos.is_empty() {
            self.show_status_state(
                "folder-music-symbolic",
                "Playlist is empty",
                "Right-click any video and choose “Playlist +” to add videos here.",
                None,
            );
            return;
        }

        let actions = self.make_card_actions();
        let layout = self.list_card_layout();
        let total = pl.videos.len();

        for (idx, vid) in pl.videos.iter().enumerate() {
            let pid = pl.id.clone();
            let weak_reorder = Rc::downgrade(self);
            let weak_remove = Rc::downgrade(self);
            let pid_remove = pl.id.clone();

            let card = VideoCard::new_playlist_track(
                vid,
                idx,
                total,
                layout,
                true,
                move |from, to| {
                    if let Some(w) = weak_reorder.upgrade() {
                        if w.playlist_store.move_video(&pid, from, to) {
                            w.open_saved_playlist(&pid);
                            w.highlight_reordered_playlist_row(to);
                            w.focus_playlist_reorder_row(to);
                        }
                    }
                },
                move || {
                    if let Some(w) = weak_remove.upgrade() {
                        if let Some(removed_video) = w.playlist_store.remove_video_at(&pid_remove, idx)
                        {
                            w.open_saved_playlist(&pid_remove);

                            let toast = adw::Toast::new("Removed video from playlist");
                            toast.set_button_label(Some("Undo"));
                            let weak_undo = Rc::downgrade(&w);
                            let pid_undo = pid_remove.clone();
                            toast.connect_button_clicked(move |_| {
                                if let Some(w) = weak_undo.upgrade() {
                                    if w.playlist_store.insert_video_at(
                                        &pid_undo,
                                        idx,
                                        removed_video.clone(),
                                    ) {
                                        w.open_saved_playlist(&pid_undo);
                                    }
                                }
                            });
                            w.toast_overlay.add_toast(toast);
                        }
                    }
                },
            );

            self.wire_playlist_track_interactions(&card, vid, idx, &actions);
            self.flow_box.append(&card.container);
        }
    }

    fn highlight_reordered_playlist_row(&self, index: usize) {
        let Some(child) = self.flow_box.child_at_index(index as i32) else {
            return;
        };
        let Some(row) = child.child() else {
            return;
        };
        row.add_css_class("reordered");
        let row_weak = row.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(420), move || {
            if let Some(row) = row_weak.upgrade() {
                row.remove_css_class("reordered");
            }
        });
    }

    fn focus_playlist_reorder_row(self: &Rc<Self>, index: usize) {
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(win) = weak.upgrade() else {
                return;
            };
            if let Some(child) = win.flow_box.child_at_index(index as i32) {
                if let Some(row) = child.child() {
                    if let Some(handle) = row.first_child() {
                        handle.grab_focus();
                    }
                }
            }
        });
    }

    pub fn open_remote_playlist(self: &Rc<Self>, playlist_id: &str) {
        let req_id = self.next_request_id();
        self.prepare_view_shell(
            ActiveView::RemotePlaylist(playlist_id.to_string()),
            ViewMode::SearchList,
            false,
            false,
            false,
            None,
        );

        self.playlist_title_lbl.set_text("Loading playlist…");
        self.playlist_meta_lbl.set_text("");
        self.playlist_rename_btn.set_visible(false);
        self.playlist_delete_btn.set_visible(false);
        self.playlist_save_remote_btn.set_visible(true);
        self.playlist_save_remote_btn.set_label(
            if self
                .playlist_store
                .find_by_youtube_id(playlist_id)
                .is_some()
            {
                "Saved ✓"
            } else {
                "Save playlist"
            },
        );

        self.show_skeletons_search(5);

        let engine = self.engine.clone();
        let pid = playlist_id.to_string();
        let pid_retry = playlist_id.to_string();
        let (tx, rx) = async_channel::bounded::<Result<RemotePlaylist, String>>(1);
        self.runtime.spawn(async move {
            let res = engine.fetch_playlist(&pid).await;
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
            match res {
                Ok(pl) => {
                    *w.feed_cursor.borrow_mut() = pl.next.clone();
                    *w.remote_playlist_data.borrow_mut() = Some(pl.clone());
                    w.render_remote_playlist_view(&pl);
                }
                Err(err) => {
                    let wr = Rc::downgrade(&w);
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Could not open playlist",
                        &err,
                        Some((
                            "Retry",
                            Rc::new(move || {
                                if let Some(win) = wr.upgrade() {
                                    win.open_remote_playlist(&pid_retry);
                                }
                            }),
                        )),
                    );
                }
            }
        });
    }

    fn render_remote_playlist_view(self: &Rc<Self>, pl: &RemotePlaylist) {
        self.clear_card_selection();
        self.playlist_title_lbl.set_text(&pl.name);
        let meta = if pl.channel_name.is_empty() {
            pl.video_count_text.clone()
        } else {
            format!("{} · {}", pl.channel_name, pl.video_count_text)
        };
        self.playlist_meta_lbl.set_text(&meta);

        self.clear_home_flow_box();
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        *self.current_videos.borrow_mut() = Vec::new();
        self.append_remote_playlist_tracks(&pl.videos);
    }

    fn append_remote_playlist_tracks(self: &Rc<Self>, videos: &[VideoSummary]) {
        let actions = self.make_card_actions();
        let layout = self.list_card_layout();
        let start_idx = self.current_videos.borrow().len();

        for (offset, vid) in videos.iter().enumerate() {
            let idx = start_idx + offset;
            let card = VideoCard::new_playlist_track(vid, idx, 0, layout, false, |_, _| {}, || {});
            self.wire_playlist_track_interactions(&card, vid, idx, &actions);
            self.flow_box.append(&card.container);
            self.current_videos.borrow_mut().push(vid.clone());
        }
    }

    fn wire_playlist_track_interactions(
        self: &Rc<Self>,
        card: &VideoCard,
        vid: &VideoSummary,
        idx: usize,
        actions: &CardActions,
    ) {
        self.bind_home_picture(&vid.id, &vid.thumbnail_url, &card.picture);

        let vid_id = vid.id.clone();
        let vid_id_det = vid.id.clone();
        let weak_play = Rc::downgrade(self);
        let play_cb: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(w) = weak_play.upgrade() {
                let all_vids = w.current_videos.borrow().clone();
                if !all_vids.is_empty() {
                    let target_idx = all_vids
                        .iter()
                        .position(|v| v.id == vid_id)
                        .unwrap_or(idx);
                    let q = Quality::from_setting(&w.settings_store.load().default_quality);
                    let cont = w.remote_playlist_queue_continuation();
                    w.play_queue_from_with_continuation(all_vids, target_idx, q, false, cont);
                }
            }
        });

        let weak_sel = Rc::downgrade(self);
        let weak_det_play = Rc::downgrade(self);
        let play_enter = play_cb.clone();
        let play_dbl = play_cb;

        let weak_det = Rc::downgrade(self);
        let sum_det = vid.clone();
        let mut act_det = actions.clone();
        act_det.on_play = Rc::new(move |_summary, q| {
            if let Some(w) = weak_det_play.upgrade() {
                let all_vids = w.current_videos.borrow().clone();
                if !all_vids.is_empty() {
                    let target_idx = all_vids
                        .iter()
                        .position(|v| v.id == vid_id_det)
                        .unwrap_or(idx);
                    let cont = w.remote_playlist_queue_continuation();
                    w.play_queue_from_with_continuation(all_vids, target_idx, q, false, cont);
                }
            }
        });

        card.connect_card_interactions(
            move |hl| {
                if let Some(w) = weak_sel.upgrade() {
                    w.select_card(hl, play_enter.clone());
                }
            },
            move || play_dbl(),
            move || {
                if let Some(w) = weak_det.upgrade() {
                    DetailsSheet::show(&w.window, &sum_det, &act_det);
                }
            },
        );
    }

    pub fn search_category(self: &Rc<Self>, category: &str) {
        self.search_category_opts(category, false);
    }

    pub fn search_category_opts(self: &Rc<Self>, category: &str, force_refresh: bool) {
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
            let res = engine.category_feed(&cat, force_refresh).await;
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
                        *w.feed_cursor.borrow_mut() = paged.next;
                        w.populate_videos(paged.items, ViewMode::Grid);
                    }
                }
                Err(e) => {
                    let weak_retry = Rc::downgrade(&w);
                    let cat_retry = cat_ui.clone();
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Network Error",
                        &e,
                        Some((
                            "Retry",
                            Rc::new(move || {
                                if let Some(win) = weak_retry.upgrade() {
                                    win.search_category(&cat_retry);
                                }
                            }),
                        )),
                    );
                }
            }
        });
    }

    fn load_more_current_feed(self: &Rc<Self>) {
        if self.is_loading_more.get() {
            return;
        }

        let av = self.active_view.borrow().clone();
        let req_id = self.request_seq.get();

        match av {
            ActiveView::HomeTrending => {
                let cursors = self.trending_cursors.borrow().clone();
                if cursors.is_empty() {
                    return;
                }
                self.is_loading_more.set(true);
                let existing: HashSet<String> = self
                    .current_videos
                    .borrow()
                    .iter()
                    .map(|v| v.id.clone())
                    .collect();
                let engine = self.engine.clone();
                let (tx, rx) = async_channel::bounded::<Result<PagedTrending, String>>(1);
                self.runtime.spawn(async move {
                    let res = engine.trending_more(cursors, existing).await;
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
                    if let Ok(paged) = res {
                        *w.trending_cursors.borrow_mut() = paged.cursors;
                        if !paged.items.is_empty() {
                            w.append_videos(paged.items, ViewMode::Grid);
                        }
                    }
                });
            }
            ActiveView::Search(_) => {
                let Some(cursor) = self.feed_cursor.borrow().clone() else {
                    return;
                };
                self.is_loading_more.set(true);
                let tab = self.search_tab.get();
                let engine = self.engine.clone();
                let (tx, rx) = async_channel::bounded::<Result<PagedSearchItems, String>>(1);
                self.runtime.spawn(async move {
                    let res = engine.load_more_search_items(&cursor, tab).await;
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
                    if let Ok(paged) = res {
                        *w.feed_cursor.borrow_mut() = paged.next;
                        w.append_search_items(paged.items);
                    }
                });
            }
            ActiveView::HomeCategory(_) | ActiveView::RemotePlaylist(_) => {
                let Some(cursor) = self.feed_cursor.borrow().clone() else {
                    return;
                };
                self.is_loading_more.set(true);
                let is_remote_pl = matches!(av, ActiveView::RemotePlaylist(_));
                let filter_shorts = matches!(av, ActiveView::HomeCategory(_));
                let mode = self.view_mode.get();
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
                    if let Ok(paged) = res {
                        *w.feed_cursor.borrow_mut() = paged.next;
                        if is_remote_pl {
                            w.append_remote_playlist_tracks(&paged.items);
                        } else {
                            w.append_videos(paged.items, mode);
                        }
                    }
                });
            }
            _ => {}
        }
    }

    pub fn load_trending(self: &Rc<Self>) {
        self.load_trending_opts(false);
    }

    pub fn load_trending_opts(self: &Rc<Self>, force_refresh: bool) {
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

        let (sender, receiver) = async_channel::bounded::<Result<PagedTrending, String>>(1);
        let engine = self.engine.clone();

        self.runtime.spawn(async move {
            let res = engine.trending_initial(force_refresh).await;
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
                        let weak_retry = Rc::downgrade(&w);
                        w.show_status_state(
                            "system-search-symbolic",
                            "No trending videos",
                            "Check back again shortly.",
                            Some((
                                "Retry",
                                Rc::new(move || {
                                    if let Some(win) = weak_retry.upgrade() {
                                        win.load_trending();
                                    }
                                }),
                            )),
                        );
                    } else {
                        *w.trending_cursors.borrow_mut() = paged.cursors;
                        w.populate_videos(paged.items, ViewMode::Grid);
                    }
                }
                Err(e) => {
                    let weak_retry = Rc::downgrade(&w);
                    w.show_status_state(
                        "network-offline-symbolic",
                        "Network Error",
                        &e,
                        Some((
                            "Retry",
                            Rc::new(move || {
                                if let Some(win) = weak_retry.upgrade() {
                                    win.load_trending();
                                }
                            }),
                        )),
                    );
                }
            }
        });
    }

    fn navigate_to_watch_later(self: &Rc<Self>) {
        let win = self.clone();
        self.reset_nav_root(|| {
            win.clear_search_bar_quietly();
            win.set_active_nav(Some(RailTab::WatchLater));
            win.load_watch_later();
        });
    }

    pub fn load_watch_later(self: &Rc<Self>) {
        let _req_id = self.next_request_id();
        let saved = self.watch_later_store.load_all();
        self.prepare_view_shell(
            ActiveView::WatchLater,
            ViewMode::Grid,
            false,
            false,
            false,
            Some("Watch Later"),
        );
        self.clear_watch_later_btn.set_visible(!saved.is_empty());

        if saved.is_empty() {
            self.show_status_state(
                "bookmark-new-symbolic",
                "Watch Later is empty",
                "Right-click any video and choose “Watch later” to save it here.",
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
                "Open any channel or search Channels and click Subscribe to see their latest uploads here.",
                Some((
                    "Browse Trending",
                    Rc::new(move || {
                        if let Some(win) = weak_home.upgrade() {
                            win.set_active_nav(Some(RailTab::Home));
                            win.highlight_chip(Some(&win.chip_trending));
                            win.load_trending();
                        }
                    }),
                )),
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
                            Some((
                                "Refresh",
                                Rc::new(move || {
                                    if let Some(win) = weak_retry.upgrade() {
                                        win.load_subscriptions();
                                    }
                                }),
                            )),
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
                        Some((
                            "Retry",
                            Rc::new(move || {
                                if let Some(win) = weak_retry.upgrade() {
                                    win.load_subscriptions();
                                }
                            }),
                        )),
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

        self.clear_home_flow_box();

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

            self.bind_home_picture(
                &entry.video.id,
                &entry.video.thumbnail_url,
                &card.picture,
            );

            let sum_play = entry.video.clone();
            let on_play = actions.on_play.clone();
            let def_q = actions.default_quality.clone();
            let play_cb: Rc<dyn Fn()> = Rc::new(move || {
                on_play(sum_play.clone(), def_q());
            });

            let weak_sel = Rc::downgrade(self);
            let play_enter = play_cb.clone();
            let play_dbl = play_cb;

            let weak_det = Rc::downgrade(self);
            let sum_det = entry.video.clone();
            let act_det = actions.clone();

            card.connect_card_interactions(
                move |hl| {
                    if let Some(w) = weak_sel.upgrade() {
                        w.select_card(hl, play_enter.clone());
                    }
                },
                move || play_dbl(),
                move || {
                    if let Some(w) = weak_det.upgrade() {
                        DetailsSheet::show(&w.window, &sum_det, &act_det);
                    }
                },
            );

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
            .content_width(440)
            .title("Keyboard & Mouse")
            .build();
        dialog.add_css_class("yr-sheet");

        let vbox = GtkBox::new(Orientation::Vertical, 12);
        vbox.set_margin_top(20);
        vbox.set_margin_bottom(24);
        vbox.set_margin_start(24);
        vbox.set_margin_end(24);

        let heading = Label::new(Some("Controls & Shortcuts"));
        heading.add_css_class("yr-sheet-title");
        heading.set_halign(Align::Start);
        vbox.append(&heading);

        let items = [
            ("Single-click card", "Select / highlight video"),
            ("Double-click / Enter", "Play video in MPV"),
            ("Right-click card", "Open video details & options"),
            ("/ or Ctrl + F", "Focus search bar"),
            ("Ctrl + R or F5", "Refresh active feed or channel"),
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

        let ver = Label::new(Some("Version 0.2.0"));
        ver.add_css_class("yr-meta");
        vbox.append(&ver);

        let desc = Label::new(Some(
            "Fast native Linux YouTube client built with Rust, GTK4, Libadwaita, RustyPipe, and MPV — zero Python required.",
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
        self.clear_card_selection();
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        self.current_videos.borrow_mut().clear();
        self.current_search_items.borrow_mut().clear();
        self.clear_home_flow_box();
        self.view_mode.set(ViewMode::Grid);
        self.update_flow_columns();
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_skeleton());
        }
    }

    fn show_skeletons_search(self: &Rc<Self>, count: usize) {
        self.clear_card_selection();
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        self.current_videos.borrow_mut().clear();
        self.current_search_items.borrow_mut().clear();
        self.clear_home_flow_box();
        self.view_mode.set(ViewMode::SearchList);
        self.update_flow_columns();
        let layout = self.list_card_layout();
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_search_skeleton(layout));
        }
    }

    fn show_skeletons_shorts(self: &Rc<Self>, count: usize) {
        self.clear_card_selection();
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        self.current_videos.borrow_mut().clear();
        self.current_search_items.borrow_mut().clear();
        self.clear_home_flow_box();
        self.view_mode.set(ViewMode::ShortsGrid);
        self.update_flow_columns();
        for _ in 0..count {
            self.flow_box.append(&VideoCard::new_short_skeleton());
        }
    }

    fn show_status_state(
        &self,
        icon_name: &str,
        title: &str,
        description: &str,
        action: Option<(&str, Rc<dyn Fn()>)>,
    ) {
        self.clear_card_selection();
        self.clear_home_flow_box();
        self.current_videos.borrow_mut().clear();
        self.current_search_items.borrow_mut().clear();
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
        self.clear_card_selection();
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        self.clear_home_flow_box();

        self.view_mode.set(mode);
        self.update_flow_columns();
        *self.current_videos.borrow_mut() = Vec::new();
        self.append_videos(videos, mode);
    }

    fn append_videos(self: &Rc<Self>, videos: Vec<VideoSummary>, mode: ViewMode) {
        let actions = self.make_card_actions();
        let layout = self.list_card_layout();

        for summary in videos {
            self.append_single_video_card(&summary, mode, layout, &actions, None, true);
            self.current_videos.borrow_mut().push(summary);
        }
    }

    fn append_single_video_card(
        self: &Rc<Self>,
        summary: &VideoSummary,
        mode: ViewMode,
        layout: ListCardLayout,
        actions: &CardActions,
        position: Option<i32>,
        hydrate: bool,
    ) {
        let card = match mode {
            ViewMode::Grid => VideoCard::new_grid(summary),
            ViewMode::ShortsGrid => VideoCard::new_short(summary),
            ViewMode::SearchList => VideoCard::new_search(summary, layout),
        };

        let sum_play = summary.clone();
        let on_play = actions.on_play.clone();
        let def_q = actions.default_quality.clone();
        let play_cb: Rc<dyn Fn()> = Rc::new(move || {
            on_play(sum_play.clone(), def_q());
        });

        let weak_sel = Rc::downgrade(self);
        let play_enter = play_cb.clone();
        let play_dbl = play_cb;

        let weak_det = Rc::downgrade(self);
        let sum_det = summary.clone();
        let act_det = actions.clone();

        card.connect_card_interactions(
            move |hl| {
                if let Some(w) = weak_sel.upgrade() {
                    w.select_card(hl, play_enter.clone());
                }
            },
            move || play_dbl(),
            move || {
                if let Some(w) = weak_det.upgrade() {
                    DetailsSheet::show(&w.window, &sum_det, &act_det);
                }
            },
        );

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

        if mode == ViewMode::ShortsGrid {
            self.bind_home_short_picture(&summary.id, &summary.thumbnail_url, &card.picture);
        } else {
            self.bind_home_picture(&summary.id, &summary.thumbnail_url, &card.picture);
            if !summary.channel_avatar_url.is_empty() {
                self.bind_home_avatar(
                    &summary.channel_id,
                    &summary.channel_avatar_url,
                    &card.avatar,
                );
            }
            for (channel_id, avatar_url, avatar) in &card.extra_channel_avatars {
                if !avatar_url.is_empty() {
                    self.bind_home_avatar(channel_id, avatar_url, avatar);
                }
            }
        }

        if let Some(position) = position {
            self.flow_box.insert(&card.container, position);
        } else {
            self.flow_box.append(&card.container);
        }

        if hydrate && !summary.is_collaboration() && !summary.is_short {
            let (tx, rx) = async_channel::bounded(1);
            let engine = self.engine.clone();
            let original = summary.clone();
            self.runtime.spawn(async move {
                let updated = engine.enrich_video_attribution(original).await;
                let _ = tx.send(updated).await;
            });
            let weak = Rc::downgrade(self);
            let card_weak = card.container.downgrade();
            let actions = actions.clone();
            let original = summary.clone();
            let request = self.request_seq.get();
            glib::spawn_future_local(async move {
                let Ok(updated) = rx.recv().await else { return };
                if updated == original { return; }
                let (Some(w), Some(old)) = (weak.upgrade(), card_weak.upgrade()) else { return };
                if w.request_seq.get() != request || old.parent().is_none() { return; }
                let index = old.index();
                if index < 0 { return; }
                for video in w.current_videos.borrow_mut().iter_mut() {
                    if video.id == updated.id { *video = updated.clone(); }
                }
                for item in w.current_search_items.borrow_mut().iter_mut() {
                    if let SearchItem::Video(video) = item {
                        if video.id == updated.id { *video = updated.clone(); }
                    }
                }
                w.flow_box.remove(&old);
                w.append_single_video_card(&updated, mode, layout, &actions, Some(index), false);
            });
        }
    }

    pub fn present(&self) {
        self.window.present();
    }
}

fn shuffle_slice<T>(items: &mut [T]) {
    let len = items.len();
    if len <= 1 {
        return;
    }
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ PLAYLIST_SHUFFLE_SEED
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    if state == 0 {
        state = 0x9E37_79B9_7F4A_7C15;
    }
    for i in 0..len - 1 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = i + (state as usize % (len - i));
        items.swap(i, j);
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
        "Direct stream quality opened in MPV",
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
        "Save watch & search history",
        "Stored locally on this device",
        &hist_switch,
    ));
    card_data.append(&make_pref_sep());

    let clear_hist_btn = Button::with_label("Clear");
    clear_hist_btn.add_css_class("yr-btn");
    clear_hist_btn.set_valign(Align::Center);
    set_pointer(&clear_hist_btn);
    card_data.append(&make_pref_row(
        "Clear watch & search history",
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
