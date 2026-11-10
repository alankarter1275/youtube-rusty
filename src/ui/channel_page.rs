use crate::backend::engine::{
    ChannelHeaderStats, Engine, PageCursor, PagedPlaylists, PagedVideos,
};
use crate::backend::models::{
    effective_published_timestamp, matches_local_filters, ChannelDetails, ChannelSortOrder,
    DurationFilter, PlaylistSearchSummary, UploadDateFilter, VideoSummary,
};
use crate::backend::subscriptions::{SubscribedChannel, SubscriptionStore};
use crate::player::mpv::Quality;
use crate::ui::details_sheet::{CardActions, DetailsSheet};
use crate::ui::icons::{make_icon, set_pointer, KalamIcon};
use crate::ui::video_card::{ListCardLayout, PlaylistResultCard, VideoCard};
use adw::prelude::*;
use gtk4::gdk;
use gtk4::glib;
use gtk4::{
    Align, Box as GtkBox, Button, Editable, EventControllerKey, FlowBox, Label, Orientation,
    Popover, ScrolledWindow, SearchEntry, SelectionMode, Separator, ToggleButton,
};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

const VIRTUAL_WINDOW_ROWS: usize = 8;
const VIRTUAL_HYSTERESIS_ROWS: usize = 2;
const DEFAULT_SHORTS_ROW_STRIDE: f64 = 366.0;
const DEFAULT_VIDEOS_ROW_STRIDE: f64 = 264.0;
const GRID_ROW_SPACING_PX: f64 = 24.0;

pub type ShortsPlayHandler = Rc<
    dyn Fn(
        Vec<VideoSummary>,
        usize,
        Option<Quality>,
        Option<(PageCursor, Option<ChannelDetails>)>,
    ),
>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChannelTab {
    Videos,
    Shorts,
    Playlists,
}

pub struct ChannelPage {
    pub page: adw::NavigationPage,
    channel_id: String,
    initial_name: String,
    parent_window: glib::WeakRef<gtk4::Widget>,
    actions: CardActions,
    engine: Arc<Engine>,
    runtime: Arc<Runtime>,
    store: Arc<SubscriptionStore>,
    on_play_shorts_queue: ShortsPlayHandler,
    on_open_playlist: Rc<dyn Fn(String)>,
    avatar: adw::Avatar,
    name_label: Label,
    stats_label: Label,
    sub_btn: Button,
    header_box: GtkBox,
    toolbar_box: GtkBox,
    compact_search_box: GtkBox,
    tab_videos: ToggleButton,
    tab_shorts: ToggleButton,
    tab_playlists: ToggleButton,
    play_all_btn: Button,
    search_entry: SearchEntry,
    compact_search_entry: SearchEntry,
    filter_btn: Button,
    sort_btn: Button,
    refresh_btn: Button,
    scrolled: ScrolledWindow,
    top_spacer: GtkBox,
    flow_box: FlowBox,
    bottom_spacer: GtkBox,
    status_box: GtkBox,
    status_page: adw::StatusPage,
    search_more_matches_btn: Button,
    active_tab: Cell<ChannelTab>,
    is_narrow: Cell<bool>,
    is_compact: Cell<bool>,
    grid_cols: Cell<u32>,
    shorts_cols: Cell<u32>,
    is_loading: Cell<bool>,
    is_loading_more: Cell<bool>,
    filter_scan_pages: Cell<u8>,
    is_suspended: Cell<bool>,
    in_window_sync: Cell<bool>,
    playlists_loaded: Cell<bool>,
    request_seq: Cell<u64>,
    reload_seq: Cell<u64>,
    date_filter: Cell<UploadDateFilter>,
    duration_filter: Cell<DurationFilter>,
    sort_order: Cell<ChannelSortOrder>,
    channel_details: RefCell<Option<ChannelDetails>>,
    header_stats: RefCell<Option<ChannelHeaderStats>>,
    videos_list: RefCell<Vec<VideoSummary>>,
    shorts_list: RefCell<Vec<VideoSummary>>,
    playlists_list: RefCell<Vec<PlaylistSearchSummary>>,
    videos_cursor: RefCell<Option<PageCursor>>,
    shorts_cursor: RefCell<Option<PageCursor>>,
    playlists_cursor: RefCell<Option<PageCursor>>,
    search_results: RefCell<Option<Vec<VideoSummary>>>,
    rendered_items: RefCell<Vec<VideoSummary>>,
    win_start_idx: Cell<usize>,
    win_end_idx: Cell<usize>,
    shorts_row_stride: Cell<f64>,
    videos_row_stride: Cell<f64>,
    saved_scroll_y: Cell<f64>,
    selected_item_id: RefCell<Option<String>>,
    selected_box: RefCell<Option<GtkBox>>,
    selected_play_cb: RefCell<Option<Rc<dyn Fn()>>>,
}

impl ChannelPage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channel_id: &str,
        channel_name: &str,
        parent_window: &impl IsA<gtk4::Widget>,
        actions: CardActions,
        engine: Arc<Engine>,
        runtime: Arc<Runtime>,
        store: Arc<SubscriptionStore>,
        on_play_shorts_queue: ShortsPlayHandler,
        on_open_playlist: Rc<dyn Fn(String)>,
        narrow: bool,
        compact: bool,
        grid_cols: u32,
        shorts_cols: u32,
    ) -> Rc<Self> {
        let root_box = GtkBox::new(Orientation::Vertical, 0);
        root_box.set_hexpand(true);
        root_box.set_vexpand(true);

        // --- Channel Header ---
        let header_box = GtkBox::new(Orientation::Horizontal, 18);
        header_box.set_margin_start(28);
        header_box.set_margin_end(28);
        header_box.set_margin_top(20);
        header_box.set_margin_bottom(16);
        header_box.set_valign(Align::Center);

        let avatar = adw::Avatar::new(64, Some(channel_name), true);
        avatar.set_valign(Align::Center);
        header_box.append(&avatar);

        let title_col = GtkBox::new(Orientation::Vertical, 4);
        title_col.set_hexpand(true);
        title_col.set_valign(Align::Center);

        let name_label = Label::new(Some(channel_name));
        name_label.add_css_class("yr-h1");
        name_label.set_halign(Align::Start);
        name_label.set_xalign(0.0);
        name_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let stats_label = Label::new(Some("Loading channel info..."));
        stats_label.add_css_class("yr-meta");
        stats_label.set_halign(Align::Start);
        stats_label.set_xalign(0.0);
        stats_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        title_col.append(&name_label);
        title_col.append(&stats_label);
        header_box.append(&title_col);

        let sub_btn = Button::with_label("Subscribe");
        sub_btn.add_css_class("yr-btn");
        sub_btn.add_css_class("primary");
        sub_btn.set_valign(Align::Center);
        header_box.append(&sub_btn);

        root_box.append(&header_box);

        // --- Toolbar Row ---
        let toolbar_box = GtkBox::new(Orientation::Horizontal, 10);
        toolbar_box.set_margin_start(28);
        toolbar_box.set_margin_end(28);
        toolbar_box.set_margin_top(4);
        toolbar_box.set_margin_bottom(16);
        toolbar_box.set_valign(Align::Center);

        let seg_box = GtkBox::new(Orientation::Horizontal, 2);
        seg_box.add_css_class("yr-segmented");

        let tab_videos = ToggleButton::with_label("Videos");
        tab_videos.add_css_class("yr-chip");
        tab_videos.set_active(true);

        let tab_shorts = ToggleButton::with_label("Shorts");
        tab_shorts.add_css_class("yr-chip");
        tab_shorts.set_group(Some(&tab_videos));

        let tab_playlists = ToggleButton::with_label("Playlists");
        tab_playlists.add_css_class("yr-chip");
        tab_playlists.set_group(Some(&tab_videos));

        seg_box.append(&tab_videos);
        seg_box.append(&tab_shorts);
        seg_box.append(&tab_playlists);
        toolbar_box.append(&seg_box);

        let play_all_btn = Button::builder()
            .label("Play all")
            .tooltip_text("Play all Shorts in MPV")
            .visible(false)
            .build();
        play_all_btn.add_css_class("yr-btn");
        play_all_btn.add_css_class("primary");
        toolbar_box.append(&play_all_btn);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar_box.append(&spacer);

        let search_entry = SearchEntry::builder()
            .placeholder_text("Search in channel")
            .width_request(190)
            .build();
        search_entry.add_css_class("yr-search");
        toolbar_box.append(&search_entry);

        let filter_btn = Button::with_label("Filter");
        filter_btn.add_css_class("yr-chip");
        filter_btn.set_tooltip_text(Some("Filter channel videos"));
        set_pointer(&filter_btn);
        toolbar_box.append(&filter_btn);

        let sort_btn = Button::with_label("Sort: Newest");
        sort_btn.add_css_class("yr-chip");
        sort_btn.set_tooltip_text(Some("Sort channel videos"));
        set_pointer(&sort_btn);
        toolbar_box.append(&sort_btn);

        let refresh_btn = Button::new();
        refresh_btn.set_child(Some(&make_icon(KalamIcon::Refresh, 16)));
        refresh_btn.add_css_class("yr-chip");
        refresh_btn.set_tooltip_text(Some("Refresh channel (Ctrl+R / F5)"));
        set_pointer(&refresh_btn);
        toolbar_box.append(&refresh_btn);

        set_pointer(&sub_btn);
        set_pointer(&tab_videos);
        set_pointer(&tab_shorts);
        set_pointer(&tab_playlists);
        set_pointer(&play_all_btn);

        root_box.append(&toolbar_box);

        let compact_search_box = GtkBox::new(Orientation::Horizontal, 0);
        compact_search_box.set_margin_start(28);
        compact_search_box.set_margin_end(28);
        compact_search_box.set_margin_bottom(12);
        compact_search_box.set_visible(false);

        let compact_search_entry = SearchEntry::builder()
            .placeholder_text("Search in channel")
            .hexpand(true)
            .build();
        compact_search_entry.add_css_class("yr-search");
        compact_search_box.append(&compact_search_entry);
        root_box.append(&compact_search_box);

        // --- Video / Shorts Grid + Status Container ---
        let body_box = GtkBox::new(Orientation::Vertical, 0);
        body_box.set_vexpand(true);
        body_box.set_hexpand(true);
        body_box.set_margin_bottom(32);

        let status_page = adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title("No videos found")
            .description("Try changing your search or filter options.")
            .build();
        let status_box = GtkBox::new(Orientation::Vertical, 0);
        status_box.set_vexpand(true);
        status_box.set_hexpand(true);
        status_box.set_valign(Align::Center);
        status_box.set_visible(false);
        status_box.append(&status_page);
        let search_more_matches_btn = Button::with_label("Search more channel videos");
        search_more_matches_btn.add_css_class("yr-chip");
        search_more_matches_btn.add_css_class("active");
        search_more_matches_btn.set_halign(Align::Center);
        search_more_matches_btn.set_margin_top(12);
        search_more_matches_btn.set_visible(false);
        status_box.append(&search_more_matches_btn);

        let top_spacer = GtkBox::new(Orientation::Vertical, 0);
        top_spacer.set_hexpand(true);
        top_spacer.set_visible(false);

        let flow_box = FlowBox::new();
        flow_box.set_valign(Align::Start);
        flow_box.set_max_children_per_line(grid_cols);
        flow_box.set_min_children_per_line(grid_cols);
        flow_box.set_selection_mode(SelectionMode::None);
        flow_box.set_column_spacing(20);
        flow_box.set_row_spacing(24);
        flow_box.set_homogeneous(true);
        flow_box.set_margin_start(28);
        flow_box.set_margin_end(28);

        let bottom_spacer = GtkBox::new(Orientation::Vertical, 0);
        bottom_spacer.set_hexpand(true);
        bottom_spacer.set_visible(false);

        body_box.append(&status_box);
        body_box.append(&top_spacer);
        body_box.append(&flow_box);
        body_box.append(&bottom_spacer);

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&body_box)
            .vexpand(true)
            .hexpand(true)
            .build();
        scrolled.add_css_class("yr-no-scrollbar");

        root_box.append(&scrolled);

        let page = adw::NavigationPage::builder()
            .title(channel_name)
            .tag(channel_id)
            .child(&root_box)
            .build();

        if store.is_subscribed(channel_id) {
            sub_btn.set_label("Subscribed");
            sub_btn.remove_css_class("primary");
        }

        let parent_widget: gtk4::Widget = parent_window.clone().upcast();

        let this = Rc::new(Self {
            page,
            channel_id: channel_id.to_string(),
            initial_name: channel_name.to_string(),
            parent_window: parent_widget.downgrade(),
            actions,
            engine,
            runtime,
            store,
            on_play_shorts_queue,
            on_open_playlist,
            avatar,
            name_label,
            stats_label,
            sub_btn,
            header_box,
            toolbar_box,
            compact_search_box,
            tab_videos,
            tab_shorts,
            tab_playlists,
            play_all_btn,
            search_entry,
            compact_search_entry,
            filter_btn,
            sort_btn,
            refresh_btn,
            scrolled,
            top_spacer,
            flow_box,
            bottom_spacer,
            status_box,
            status_page,
            search_more_matches_btn,
            active_tab: Cell::new(ChannelTab::Videos),
            is_narrow: Cell::new(narrow),
            is_compact: Cell::new(compact),
            grid_cols: Cell::new(grid_cols),
            shorts_cols: Cell::new(shorts_cols),
            is_loading: Cell::new(true),
            is_loading_more: Cell::new(false),
            filter_scan_pages: Cell::new(0),
            is_suspended: Cell::new(false),
            in_window_sync: Cell::new(false),
            playlists_loaded: Cell::new(false),
            request_seq: Cell::new(0),
            reload_seq: Cell::new(0),
            date_filter: Cell::new(UploadDateFilter::AnyTime),
            duration_filter: Cell::new(DurationFilter::AnyLength),
            sort_order: Cell::new(ChannelSortOrder::Newest),
            channel_details: RefCell::new(None),
            header_stats: RefCell::new(None),
            videos_list: RefCell::new(Vec::new()),
            shorts_list: RefCell::new(Vec::new()),
            playlists_list: RefCell::new(Vec::new()),
            videos_cursor: RefCell::new(None),
            shorts_cursor: RefCell::new(None),
            playlists_cursor: RefCell::new(None),
            search_results: RefCell::new(None),
            rendered_items: RefCell::new(Vec::new()),
            win_start_idx: Cell::new(0),
            win_end_idx: Cell::new(0),
            shorts_row_stride: Cell::new(DEFAULT_SHORTS_ROW_STRIDE),
            videos_row_stride: Cell::new(DEFAULT_VIDEOS_ROW_STRIDE),
            saved_scroll_y: Cell::new(0.0),
            selected_item_id: RefCell::new(None),
            selected_box: RefCell::new(None),
            selected_play_cb: RefCell::new(None),
        });

        this.wire_signals(&root_box);
        this.set_layout(narrow, compact, grid_cols, shorts_cols);
        this.show_skeletons();

        this
    }

    fn wire_signals(self: &Rc<Self>, root_box: &GtkBox) {
        let weak_refresh = Rc::downgrade(self);
        self.refresh_btn.connect_clicked(move |_| {
            if let Some(cp) = weak_refresh.upgrade() {
                cp.reload();
            }
        });

        let weak_more_matches = Rc::downgrade(self);
        self.search_more_matches_btn.connect_clicked(move |_| {
            if let Some(cp) = weak_more_matches.upgrade() {
                cp.filter_scan_pages.set(0);
                cp.search_more_matches_btn.set_visible(false);
                cp.load_more_active_tab();
            }
        });

        // Enter key plays currently selected card (Capture phase so focused toolbar buttons
        // like "Play all" or tab chips cannot consume Enter when a card is highlighted)
        let key_ctrl = EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak_key = Rc::downgrade(self);
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
                let Some(cp) = weak_key.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if let Some(win) = cp.parent_window.upgrade() {
                    if let Some(root) = win.root() {
                        if let Some(focused) = root.focus() {
                            if focused.is::<Editable>()
                                || focused.ancestor(Editable::static_type()).is_some()
                            {
                                return glib::Propagation::Proceed;
                            }
                        }
                    }
                }
                let cb_opt = cp.selected_play_cb.borrow().clone();
                if let Some(cb) = cb_opt {
                    cb();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        root_box.add_controller(key_ctrl);

        // Sliding-window virtualization + near-bottom infinite scroll
        let weak_scroll = Rc::downgrade(self);
        self.scrolled
            .vadjustment()
            .connect_value_changed(move |adj| {
                let Some(cp) = weak_scroll.upgrade() else {
                    return;
                };
                cp.on_scroll_changed();
                let val = adj.value();
                let page = adj.page_size();
                let upper = adj.upper();
                if val > 0.0 && upper > page && val + page >= upper - 680.0 {
                    cp.load_more_active_tab();
                }
            });

        // Tab switching: Videos
        let weak = Rc::downgrade(self);
        self.tab_videos.connect_toggled(move |btn| {
            if btn.is_active() {
                if let Some(cp) = weak.upgrade() {
                    cp.active_tab.set(ChannelTab::Videos);
                    cp.filter_scan_pages.set(0);
                    cp.play_all_btn.set_visible(false);
                    cp.filter_btn.set_visible(true);
                    cp.sort_btn.set_visible(true);
                    cp.update_grid_columns();
                    cp.scrolled.vadjustment().set_value(0.0);
                    if cp.sort_order.get() != ChannelSortOrder::Newest
                        && cp.search_results.borrow().is_none()
                    {
                        cp.reload_sorted_tab();
                    } else {
                        cp.render_active_tab();
                    }
                }
            }
        });

        // Tab switching: Shorts
        let weak = Rc::downgrade(self);
        self.tab_shorts.connect_toggled(move |btn| {
            if btn.is_active() {
                if let Some(cp) = weak.upgrade() {
                    cp.active_tab.set(ChannelTab::Shorts);
                    cp.filter_scan_pages.set(0);
                    cp.play_all_btn.set_visible(true);
                    cp.filter_btn.set_visible(false);
                    cp.sort_btn.set_visible(true);
                    cp.update_grid_columns();
                    cp.scrolled.vadjustment().set_value(0.0);
                    if cp.sort_order.get() != ChannelSortOrder::Newest
                        && cp.search_results.borrow().is_none()
                    {
                        cp.reload_sorted_tab();
                    } else {
                        cp.render_active_tab();
                    }
                }
            }
        });

        // Tab switching: Playlists
        let weak = Rc::downgrade(self);
        self.tab_playlists.connect_toggled(move |btn| {
            if btn.is_active() {
                if let Some(cp) = weak.upgrade() {
                    cp.active_tab.set(ChannelTab::Playlists);
                    cp.play_all_btn.set_visible(false);
                    cp.filter_btn.set_visible(false);
                    cp.sort_btn.set_visible(false);
                    cp.update_grid_columns();
                    cp.scrolled.vadjustment().set_value(0.0);
                    if !cp.playlists_loaded.get() {
                        cp.fetch_playlists_tab();
                    } else {
                        cp.render_active_tab();
                    }
                }
            }
        });

        // Play all Shorts button (passes continuation cursor so remaining Shorts auto-load seamlessly)
        let weak = Rc::downgrade(self);
        self.play_all_btn.connect_clicked(move |_| {
            if let Some(cp) = weak.upgrade() {
                let items = cp.filtered_active_items();
                if items.is_empty() {
                    (cp.actions.on_toast)("No Shorts available to play".to_string());
                } else {
                    let cont = cp.shorts_continuation_payload();
                    (cp.on_play_shorts_queue)(items, 0, None, cont);
                }
            }
        });

        // Subscribe / Unsubscribe toggle
        let weak = Rc::downgrade(self);
        self.sub_btn.connect_clicked(move |btn| {
            let Some(cp) = weak.upgrade() else {
                return;
            };
            if cp.store.is_subscribed(&cp.channel_id) {
                cp.store.unsubscribe(&cp.channel_id);
                btn.set_label("Subscribe");
                btn.add_css_class("primary");
                let disp_name = cp
                    .channel_details
                    .borrow()
                    .as_ref()
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| cp.initial_name.clone());
                (cp.actions.on_toast)(format!("Unsubscribed from {disp_name}"));
            } else {
                let det = cp.channel_details.borrow();
                let name = det
                    .as_ref()
                    .map(|d| d.name.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| cp.initial_name.clone());
                let handle = det
                    .as_ref()
                    .map(|d| d.handle.clone())
                    .unwrap_or_default();
                let avatar_url = det
                    .as_ref()
                    .map(|d| d.avatar_url.clone())
                    .unwrap_or_default();
                drop(det);

                cp.store.subscribe(SubscribedChannel {
                    id: cp.channel_id.clone(),
                    name: name.clone(),
                    handle,
                    avatar_url,
                });
                btn.set_label("Subscribed");
                btn.remove_css_class("primary");
                (cp.actions.on_toast)(format!("Subscribed to {name}"));
            }
        });

        // Search in channel
        for entry in [&self.search_entry, &self.compact_search_entry] {
            let weak = Rc::downgrade(self);
            let other = if entry == &self.search_entry {
                self.compact_search_entry.clone()
            } else {
                self.search_entry.clone()
            };
            entry.connect_activate(move |ent| {
                if let Some(cp) = weak.upgrade() {
                    let q = ent.text().trim().to_string();
                    if other.text().as_str() != q {
                        other.set_text(&q);
                    }
                    if q.is_empty() {
                        *cp.search_results.borrow_mut() = None;
                        cp.render_active_tab();
                    } else {
                        cp.run_channel_search(&q);
                    }
                }
            });

            let weak = Rc::downgrade(self);
            let other_clear = if entry == &self.search_entry {
                self.compact_search_entry.clone()
            } else {
                self.search_entry.clone()
            };
            entry.connect_search_changed(move |ent| {
                if ent.text().trim().is_empty() {
                    if !other_clear.text().is_empty() {
                        other_clear.set_text("");
                    }
                    if let Some(cp) = weak.upgrade() {
                        if cp.search_results.borrow().is_some() {
                            *cp.search_results.borrow_mut() = None;
                            cp.render_active_tab();
                        }
                    }
                }
            });
        }

        self.setup_filter_popover();
        self.setup_sort_popover();
    }

    fn select_card_box(&self, item_id: Option<&str>, target: &GtkBox, play_cb: Rc<dyn Fn()>) {
        if let Some(prev) = self.selected_box.borrow_mut().replace(target.clone()) {
            if prev != *target {
                prev.remove_css_class("selected");
            }
        }
        target.add_css_class("selected");
        *self.selected_item_id.borrow_mut() = item_id.map(str::to_string);
        *self.selected_play_cb.borrow_mut() = Some(play_cb);
    }

    fn clear_selection(&self) {
        if let Some(prev) = self.selected_box.borrow_mut().take() {
            prev.remove_css_class("selected");
        }
        *self.selected_item_id.borrow_mut() = None;
        *self.selected_play_cb.borrow_mut() = None;
    }

    fn setup_filter_popover(self: &Rc<Self>) {
        let popover = Popover::new();
        popover.set_parent(&self.filter_btn);
        popover.set_has_arrow(false);

        let pop_cleanup = popover.clone();
        self.filter_btn.connect_destroy(move |_| {
            pop_cleanup.unparent();
        });

        let content = GtkBox::new(Orientation::Vertical, 2);
        content.set_width_request(210);

        let h1 = Label::new(Some("UPLOAD DATE"));
        h1.add_css_class("yr-popover-head");
        h1.set_halign(Align::Start);
        content.append(&h1);

        let date_specs = [
            (UploadDateFilter::AnyTime, "Any time"),
            (UploadDateFilter::Today, "Today"),
            (UploadDateFilter::ThisWeek, "This week"),
            (UploadDateFilter::ThisMonth, "This month"),
            (UploadDateFilter::ThisYear, "This year"),
        ];

        let pending_date = Rc::new(Cell::new(UploadDateFilter::AnyTime));
        let mut date_buttons: Vec<(UploadDateFilter, &'static str, Button)> = Vec::new();
        for (val, label) in date_specs {
            let init_text = if val == UploadDateFilter::AnyTime {
                format!("{label} ✓")
            } else {
                label.to_string()
            };
            let btn = create_pop_item(&init_text);
            if val == UploadDateFilter::AnyTime {
                btn.add_css_class("selected");
            }
            content.append(&btn);
            date_buttons.push((val, label, btn));
        }
        let date_buttons_rc = Rc::new(date_buttons);

        for (val, _, btn) in date_buttons_rc.iter() {
            let v = *val;
            let pd = pending_date.clone();
            let all = date_buttons_rc.clone();
            btn.connect_clicked(move |_| {
                pd.set(v);
                for (item_val, base_lbl, b) in all.iter() {
                    if *item_val == v {
                        set_pop_item_text(b, &format!("{base_lbl} ✓"), true);
                    } else {
                        set_pop_item_text(b, base_lbl, false);
                    }
                }
            });
        }

        let sep = Separator::new(Orientation::Horizontal);
        sep.set_margin_top(4);
        sep.set_margin_bottom(4);
        content.append(&sep);

        let h2 = Label::new(Some("DURATION"));
        h2.add_css_class("yr-popover-head");
        h2.set_halign(Align::Start);
        content.append(&h2);

        let dur_specs = [
            (DurationFilter::AnyLength, "Any length"),
            (DurationFilter::Under4Min, "Under 4 min"),
            (DurationFilter::FourTo20Min, "4 to 20 min"),
            (DurationFilter::Over20Min, "Over 20 min"),
        ];

        let pending_dur = Rc::new(Cell::new(DurationFilter::AnyLength));
        let mut dur_buttons: Vec<(DurationFilter, &'static str, Button)> = Vec::new();
        for (val, label) in dur_specs {
            let init_text = if val == DurationFilter::AnyLength {
                format!("{label} ✓")
            } else {
                label.to_string()
            };
            let btn = create_pop_item(&init_text);
            if val == DurationFilter::AnyLength {
                btn.add_css_class("selected");
            }
            content.append(&btn);
            dur_buttons.push((val, label, btn));
        }
        let dur_buttons_rc = Rc::new(dur_buttons);

        for (val, _, btn) in dur_buttons_rc.iter() {
            let v = *val;
            let pdur = pending_dur.clone();
            let all = dur_buttons_rc.clone();
            btn.connect_clicked(move |_| {
                pdur.set(v);
                for (item_val, base_lbl, b) in all.iter() {
                    if *item_val == v {
                        set_pop_item_text(b, &format!("{base_lbl} ✓"), true);
                    } else {
                        set_pop_item_text(b, base_lbl, false);
                    }
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
        let pd_reset = pending_date.clone();
        let pdur_reset = pending_dur.clone();
        let db_reset = date_buttons_rc.clone();
        let durb_reset = dur_buttons_rc.clone();
        reset_btn.connect_clicked(move |_| {
            pd_reset.set(UploadDateFilter::AnyTime);
            pdur_reset.set(DurationFilter::AnyLength);
            for (v, lbl, b) in db_reset.iter() {
                let sel = *v == UploadDateFilter::AnyTime;
                set_pop_item_text(
                    b,
                    &if sel {
                        format!("{lbl} ✓")
                    } else {
                        lbl.to_string()
                    },
                    sel,
                );
            }
            for (v, lbl, b) in durb_reset.iter() {
                let sel = *v == DurationFilter::AnyLength;
                set_pop_item_text(
                    b,
                    &if sel {
                        format!("{lbl} ✓")
                    } else {
                        lbl.to_string()
                    },
                    sel,
                );
            }
            if let Some(pop) = pop_reset.upgrade() {
                pop.popdown();
            }
            if let Some(cp) = weak_reset.upgrade() {
                cp.filter_scan_pages.set(0);
                cp.date_filter.set(UploadDateFilter::AnyTime);
                cp.duration_filter.set(DurationFilter::AnyLength);
                cp.filter_btn.remove_css_class("active");
                cp.render_active_tab();
            }
        });

        let weak_apply = Rc::downgrade(self);
        let pop_apply = popover.downgrade();
        let pd_apply = pending_date;
        let pdur_apply = pending_dur.clone();
        apply_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_apply.upgrade() {
                pop.popdown();
            }
            if let Some(cp) = weak_apply.upgrade() {
                cp.filter_scan_pages.set(0);
                let df = pd_apply.get();
                let duf = pdur_apply.get();
                cp.date_filter.set(df);
                cp.duration_filter.set(duf);
                if df != UploadDateFilter::AnyTime || duf != DurationFilter::AnyLength {
                    cp.filter_btn.add_css_class("active");
                } else {
                    cp.filter_btn.remove_css_class("active");
                }
                cp.render_active_tab();
            }
        });

        let pop_weak = popover.downgrade();
        let weak_cp = Rc::downgrade(self);
        let duration_header = h2.clone();
        let duration_separator = sep.clone();
        let duration_buttons = dur_buttons_rc.clone();
        let pending_dur_open = pending_dur.clone();
        self.filter_btn.connect_clicked(move |_| {
            if let Some(cp) = weak_cp.upgrade() {
                let show_duration = cp.active_tab.get() != ChannelTab::Shorts;
                duration_header.set_visible(show_duration);
                duration_separator.set_visible(show_duration);
                for (_, _, button) in duration_buttons.iter() {
                    button.set_visible(show_duration);
                }
                if !show_duration {
                    pending_dur_open.set(DurationFilter::AnyLength);
                    for (value, label, button) in duration_buttons.iter() {
                        let selected = *value == DurationFilter::AnyLength;
                        set_pop_item_text(
                            button,
                            &if selected { format!("{label} ✓") } else { label.to_string() },
                            selected,
                        );
                    }
                }
            }
            if let Some(pop) = pop_weak.upgrade() {
                pop.popup();
            }
        });
    }

    fn setup_sort_popover(self: &Rc<Self>) {
        let popover = Popover::new();
        popover.set_parent(&self.sort_btn);
        popover.set_has_arrow(false);

        let pop_cleanup = popover.clone();
        self.sort_btn.connect_destroy(move |_| {
            pop_cleanup.unparent();
        });

        let content = GtkBox::new(Orientation::Vertical, 2);
        content.set_width_request(180);

        let h = Label::new(Some("SORT BY"));
        h.add_css_class("yr-popover-head");
        h.set_halign(Align::Start);
        content.append(&h);

        let specs = [
            (ChannelSortOrder::Newest, "Newest"),
            (ChannelSortOrder::Oldest, "Oldest"),
            (ChannelSortOrder::Popular, "Most popular"),
        ];

        let mut sort_buttons: Vec<(ChannelSortOrder, &'static str, Button)> = Vec::new();
        for (ord, lbl) in specs {
            let init_text = if ord == ChannelSortOrder::Newest {
                format!("{lbl} ✓")
            } else {
                lbl.to_string()
            };
            let btn = create_pop_item(&init_text);
            if ord == ChannelSortOrder::Newest {
                btn.add_css_class("selected");
            }
            content.append(&btn);
            sort_buttons.push((ord, lbl, btn));
        }
        let sort_buttons_rc = Rc::new(sort_buttons);

        for (ord, _, btn) in sort_buttons_rc.iter() {
            let chosen = *ord;
            let all = sort_buttons_rc.clone();
            let pop_weak = popover.downgrade();
            let weak_cp = Rc::downgrade(self);
            btn.connect_clicked(move |_| {
                for (item_ord, base_lbl, b) in all.iter() {
                    if *item_ord == chosen {
                        set_pop_item_text(b, &format!("{base_lbl} ✓"), true);
                    } else {
                        set_pop_item_text(b, base_lbl, false);
                    }
                }
                if let Some(pop) = pop_weak.upgrade() {
                    pop.popdown();
                }
                if let Some(cp) = weak_cp.upgrade() {
                    if cp.sort_order.get() != chosen {
                        cp.filter_scan_pages.set(0);
                        cp.sort_order.set(chosen);
                        cp.sort_btn.set_label(chosen.button_label());
                        if cp.search_results.borrow().is_some() {
                            cp.render_active_tab();
                        } else {
                            cp.reload_sorted_tab();
                        }
                    }
                }
            });
        }

        popover.set_child(Some(&content));

        let pop_weak = popover.downgrade();
        self.sort_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popup();
            }
        });
    }

    fn next_request_id(&self) -> u64 {
        let next = self.request_seq.get().wrapping_add(1);
        self.request_seq.set(next);
        next
    }

    fn reload_sorted_tab(self: &Rc<Self>) {
        if self.search_results.borrow().is_some() {
            self.render_active_tab();
            return;
        }
        let req_id = self.next_request_id();
        self.is_loading.set(true);
        self.show_skeletons();

        let is_shorts = self.active_tab.get() == ChannelTab::Shorts;
        let order = self.sort_order.get();
        let channel_id = self.channel_id.clone();
        let fallback = self.channel_details.borrow().clone();
        let engine = self.engine.clone();
        let (tx, rx) = async_channel::bounded::<Result<PagedVideos, String>>(1);
        self.runtime.spawn(async move {
            let result = engine
                .channel_tab_ordered(&channel_id, is_shorts, order, fallback.as_ref())
                .await;
            let _ = tx.send(result).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else {
                return;
            };
            let Some(cp) = weak.upgrade() else {
                return;
            };
            if cp.request_seq.get() != req_id {
                return;
            }
            cp.is_loading.set(false);
            match result {
                Ok(paged) => {
                    if is_shorts {
                        *cp.shorts_list.borrow_mut() = paged.items;
                        *cp.shorts_cursor.borrow_mut() = paged.next;
                    } else {
                        *cp.videos_list.borrow_mut() = paged.items;
                        *cp.videos_cursor.borrow_mut() = paged.next;
                    }
                }
                Err(err) => {
                    // Keep the current items, but don't silently imply the requested order loaded.
                    (cp.actions.on_toast)(format!("Could not load sorted channel videos: {err}"));
                }
            }
            cp.render_active_tab();
        });
    }

    fn shorts_continuation_payload(&self) -> Option<(PageCursor, Option<ChannelDetails>)> {
        if self.search_results.borrow().is_some()
            || self.date_filter.get() != UploadDateFilter::AnyTime
            || self.duration_filter.get() != DurationFilter::AnyLength
        {
            return None;
        }
        self.shorts_cursor
            .borrow()
            .clone()
            .map(|c| (c, self.channel_details.borrow().clone()))
    }

    fn fetch_playlists_tab(self: &Rc<Self>) {
        let req_id = self.next_request_id();
        self.is_loading.set(true);
        self.show_skeletons();

        let cid = self.channel_id.clone();
        let fallback_name = self
            .channel_details
            .borrow()
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_else(|| self.initial_name.clone());
        let engine = self.engine.clone();
        let (tx, rx) = async_channel::bounded::<Result<PagedPlaylists, String>>(1);

        self.runtime.spawn(async move {
            let res = engine.channel_playlists(&cid, &fallback_name).await;
            let _ = tx.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else {
                return;
            };
            let Some(cp) = weak.upgrade() else {
                return;
            };
            if cp.request_seq.get() != req_id {
                return;
            }
            cp.is_loading.set(false);
            cp.playlists_loaded.set(true);
            match res {
                Ok(paged) => {
                    *cp.playlists_list.borrow_mut() = paged.items;
                    *cp.playlists_cursor.borrow_mut() = paged.next;
                    cp.render_active_tab();
                }
                Err(err) => {
                    cp.show_status("Could not load channel playlists", &err);
                }
            }
        });
    }

    fn load_more_active_tab(self: &Rc<Self>) {
        if self.is_loading.get()
            || self.is_loading_more.get()
            || self.search_results.borrow().is_some()
        {
            return;
        }

        if self.active_tab.get() == ChannelTab::Playlists {
            let Some(cursor) = self.playlists_cursor.borrow().clone() else {
                return;
            };
            self.is_loading_more.set(true);
            let fallback_name = self
                .channel_details
                .borrow()
                .as_ref()
                .map(|d| d.name.clone())
                .unwrap_or_else(|| self.initial_name.clone());
            let engine = self.engine.clone();
            let (tx, rx) = async_channel::bounded::<Result<PagedPlaylists, String>>(1);
            self.runtime.spawn(async move {
                let res = engine
                    .continue_channel_playlists(&cursor, &fallback_name)
                    .await;
                let _ = tx.send(res).await;
            });

            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let Ok(res) = rx.recv().await else {
                    return;
                };
                let Some(cp) = weak.upgrade() else {
                    return;
                };
                cp.is_loading_more.set(false);
                if let Ok(paged) = res {
                    let existing: HashSet<String> = cp
                        .playlists_list
                        .borrow()
                        .iter()
                        .map(|p| p.id.clone())
                        .collect();
                    let unique_pls: Vec<PlaylistSearchSummary> = paged
                        .items
                        .into_iter()
                        .filter(|p| !existing.contains(&p.id))
                        .collect();
                    cp.playlists_list.borrow_mut().extend(unique_pls.clone());
                    *cp.playlists_cursor.borrow_mut() = paged.next;
                    if cp.active_tab.get() == ChannelTab::Playlists
                        && cp.search_results.borrow().is_none()
                        && !cp.is_suspended.get()
                    {
                        cp.append_playlists_to_flow(&unique_pls);
                    }
                }
            });
            return;
        }

        let is_shorts = self.active_tab.get() == ChannelTab::Shorts;
        let cursor_opt = if is_shorts {
            self.shorts_cursor.borrow().clone()
        } else {
            self.videos_cursor.borrow().clone()
        };
        let Some(cursor) = cursor_opt else {
            return;
        };

        self.is_loading_more.set(true);

        let fallback = self.channel_details.borrow().clone();
        let engine = self.engine.clone();
        let (tx, rx) = async_channel::bounded::<Result<PagedVideos, String>>(1);
        self.runtime.spawn(async move {
            let res = engine
                .load_more(&cursor, fallback.as_ref(), is_shorts, false)
                .await;
            let _ = tx.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else {
                return;
            };
            let Some(cp) = weak.upgrade() else {
                return;
            };
            cp.is_loading_more.set(false);

            match res {
                Ok(mut paged) => {
                    if let Some(ref details) = *cp.channel_details.borrow() {
                        for item in &mut paged.items {
                            if item.channel_id.is_empty() {
                                item.channel_id = details.id.clone();
                            }
                            if item.channel_name.is_empty() {
                                item.channel_name = details.name.clone();
                            }
                            if item.channel_avatar_url.is_empty() {
                                item.channel_avatar_url = details.avatar_url.clone();
                            }
                        }
                    }

                    if is_shorts {
                        let existing: HashSet<String> = cp
                            .shorts_list
                            .borrow()
                            .iter()
                            .map(|v| v.id.clone())
                            .collect();
                        let mut seen = existing;
                        let unique_items: Vec<VideoSummary> = paged
                            .items
                            .into_iter()
                            .filter(|v| !v.id.is_empty() && seen.insert(v.id.clone()))
                            .collect();
                        cp.shorts_list.borrow_mut().extend(unique_items);
                        *cp.shorts_cursor.borrow_mut() = paged.next;
                        if cp.active_tab.get() == ChannelTab::Shorts
                            && cp.search_results.borrow().is_none()
                            && !cp.is_suspended.get()
                        {
                            cp.render_active_tab();
                        }
                    } else {
                        let existing: HashSet<String> = cp
                            .videos_list
                            .borrow()
                            .iter()
                            .map(|v| v.id.clone())
                            .collect();
                        let mut seen = existing;
                        let unique_items: Vec<VideoSummary> = paged
                            .items
                            .into_iter()
                            .filter(|v| !v.id.is_empty() && seen.insert(v.id.clone()))
                            .collect();
                        cp.videos_list.borrow_mut().extend(unique_items);
                        *cp.videos_cursor.borrow_mut() = paged.next;
                        if cp.active_tab.get() == ChannelTab::Videos
                            && cp.search_results.borrow().is_none()
                            && !cp.is_suspended.get()
                        {
                            cp.render_active_tab();
                        }
                    }
                }
                Err(err) => {
                    (cp.actions.on_toast)(err);
                    let has_filter = has_active_channel_filters(
                        cp.active_tab.get(),
                        cp.date_filter.get(),
                        cp.duration_filter.get(),
                    );
                    if has_filter && cp.search_results.borrow().is_none() {
                        cp.show_status(
                            "Could not check more videos",
                            "The filter couldn’t load another page. You can retry or clear the filters.",
                        );
                        cp.search_more_matches_btn.set_visible(true);
                    }
                }
            }
        });
    }

    fn run_channel_search(self: &Rc<Self>, query: &str) {
        let req_id = self.next_request_id();
        self.is_loading.set(true);
        self.show_skeletons();

        let cid = self.channel_id.clone();
        let q = query.to_string();
        let fallback = self.channel_details.borrow().clone();
        let engine = self.engine.clone();
        let (tx, rx) = async_channel::bounded::<Result<Vec<VideoSummary>, String>>(1);

        self.runtime.spawn(async move {
            let res = engine.channel_search(&cid, &q, fallback.as_ref()).await;
            let _ = tx.send(res).await;
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else {
                return;
            };
            let Some(cp) = weak.upgrade() else {
                return;
            };
            if cp.request_seq.get() != req_id {
                return;
            }
            cp.is_loading.set(false);
            match res {
                Ok(items) => {
                    *cp.search_results.borrow_mut() = Some(items);
                    cp.render_active_tab();
                }
                Err(err) => {
                    cp.show_status("Channel search failed", &err);
                }
            }
        });
    }

    pub fn set_layout(
        self: &Rc<Self>,
        narrow: bool,
        compact: bool,
        grid_cols: u32,
        shorts_cols: u32,
    ) {
        let cols_changed =
            self.grid_cols.get() != grid_cols || self.shorts_cols.get() != shorts_cols;
        self.is_narrow.set(narrow);
        self.is_compact.set(compact);
        self.grid_cols.set(grid_cols);
        self.shorts_cols.set(shorts_cols);

        let show_compact_search = narrow || compact;
        self.search_entry.set_visible(!show_compact_search);
        self.compact_search_box.set_visible(show_compact_search);

        let side = if narrow {
            16
        } else if compact {
            20
        } else {
            28
        };
        self.header_box.set_margin_start(side);
        self.header_box.set_margin_end(side);
        self.toolbar_box.set_margin_start(side);
        self.toolbar_box.set_margin_end(side);
        self.compact_search_box.set_margin_start(side);
        self.compact_search_box.set_margin_end(side);
        self.flow_box.set_margin_start(side);
        self.flow_box.set_margin_end(side);
        self.update_grid_columns();

        if cols_changed
            && !self.is_loading.get()
            && !self.is_suspended.get()
            && !self.rendered_items.borrow().is_empty()
        {
            self.sync_virtual_window(true);
        }
    }

    fn update_grid_columns(&self) {
        if self.active_tab.get() == ChannelTab::Playlists && self.search_results.borrow().is_none() {
            self.flow_box.set_homogeneous(false);
            self.flow_box.set_min_children_per_line(1);
            self.flow_box.set_max_children_per_line(1);
            self.flow_box.set_column_spacing(0);
            return;
        }
        self.flow_box.set_homogeneous(true);
        match self.active_tab.get() {
            ChannelTab::Videos | ChannelTab::Playlists => {
                let cols = self.grid_cols.get();
                self.flow_box.set_min_children_per_line(cols);
                self.flow_box.set_max_children_per_line(cols);
                self.flow_box.set_column_spacing(20);
            }
            ChannelTab::Shorts => {
                let cols = self.shorts_cols.get();
                self.flow_box.set_min_children_per_line(cols);
                self.flow_box.set_max_children_per_line(cols);
                self.flow_box.set_column_spacing(14);
            }
        }
    }

    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    pub fn reload(self: &Rc<Self>) {
        let seq = self.reload_seq.get().wrapping_add(1);
        self.reload_seq.set(seq);
        let req_seq = self.request_seq.get().wrapping_add(1);
        self.request_seq.set(req_seq);

        self.is_loading.set(true);
        self.is_loading_more.set(false);
        self.playlists_loaded.set(false);
        *self.search_results.borrow_mut() = None;
        self.search_entry.set_text("");
        self.compact_search_entry.set_text("");
        self.sort_order.set(ChannelSortOrder::Newest);
        self.sort_btn.set_label("Sort: Newest");
        self.show_skeletons();

        if self.active_tab.get() == ChannelTab::Playlists {
            self.fetch_playlists_tab();
        }

        let (ch_tx, ch_rx) =
            async_channel::bounded::<Result<(ChannelDetails, PagedVideos), String>>(1);
        let (sh_tx, sh_rx) = async_channel::bounded::<
            Result<(Option<ChannelHeaderStats>, PagedVideos), String>,
        >(1);

        let engine = self.engine.clone();
        let cid = self.channel_id.clone();
        self.runtime.spawn(async move {
            let res = engine.channel_videos(&cid).await;
            let _ = ch_tx.send(res).await;
        });

        let engine = self.engine.clone();
        let cid2 = self.channel_id.clone();
        self.runtime.spawn(async move {
            let res = engine.channel_shorts_with_stats(&cid2, None).await;
            let _ = sh_tx.send(res).await;
        });

        let cp_weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if let Ok(res) = ch_rx.recv().await {
                if let Some(cp) = cp_weak.upgrade() {
                    if cp.reload_seq.get() != seq {
                        return;
                    }
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

        let cp_weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if let Ok(Ok((stats_opt, paged))) = sh_rx.recv().await {
                if let Some(cp) = cp_weak.upgrade() {
                    if cp.reload_seq.get() != seq {
                        return;
                    }
                    if let Some(stats) = stats_opt {
                        cp.apply_header_stats(stats);
                    }
                    cp.set_shorts(paged);
                }
            }
        });
    }

    fn update_stats_label(&self, details: &ChannelDetails) {
        let stats_parts: Vec<&str> = [
            details.handle.as_str(),
            details.subscriber_count_text.as_str(),
            details.video_count_text.as_str(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
        self.stats_label.set_text(&stats_parts.join(" · "));
    }

    pub fn apply_header_stats(&self, stats: ChannelHeaderStats) {
        if let Some(ref mut details) = *self.channel_details.borrow_mut() {
            if let Some(ref h) = stats.handle {
                if !h.is_empty() {
                    details.handle = h.clone();
                }
            }
            if let Some(ref subs) = stats.subscriber_count_text {
                if !subs.is_empty() {
                    details.subscriber_count_text = subs.clone();
                }
            }
            if let Some(ref vids) = stats.video_count_text {
                if !vids.is_empty() {
                    details.video_count_text = vids.clone();
                }
            }
            self.update_stats_label(details);
        }
        *self.header_stats.borrow_mut() = Some(stats);
    }

    pub fn set_channel_details(&self, mut details: ChannelDetails) {
        if let Some(ref stats) = *self.header_stats.borrow() {
            if let Some(ref h) = stats.handle {
                if !h.is_empty() {
                    details.handle = h.clone();
                }
            }
            if let Some(ref subs) = stats.subscriber_count_text {
                if !subs.is_empty() {
                    details.subscriber_count_text = subs.clone();
                }
            }
            if let Some(ref vids) = stats.video_count_text {
                if !vids.is_empty() {
                    details.video_count_text = vids.clone();
                }
            }
        }

        self.name_label.set_text(&details.name);
        self.page.set_title(&details.name);
        self.avatar.set_text(Some(&details.name));
        self.update_stats_label(&details);

        if !details.avatar_url.is_empty() {
            self.actions
                .image_loader
                .load_into_avatar(&details.id, &details.avatar_url, &self.avatar);
        }

        if self.store.is_subscribed(&details.id) {
            self.store.subscribe(SubscribedChannel {
                id: details.id.clone(),
                name: details.name.clone(),
                handle: details.handle.clone(),
                avatar_url: details.avatar_url.clone(),
            });
        }

        for short in self.shorts_list.borrow_mut().iter_mut() {
            if short.channel_id.is_empty() {
                short.channel_id = details.id.clone();
            }
            if short.channel_name.is_empty() {
                short.channel_name = details.name.clone();
            }
            if short.channel_avatar_url.is_empty() {
                short.channel_avatar_url = details.avatar_url.clone();
            }
        }

        *self.channel_details.borrow_mut() = Some(details);
    }

    pub fn set_videos(self: &Rc<Self>, paged: PagedVideos) {
        self.is_loading.set(false);
        *self.videos_list.borrow_mut() = paged.items;
        *self.videos_cursor.borrow_mut() = paged.next;
        if self.active_tab.get() == ChannelTab::Videos && self.search_results.borrow().is_none() {
            self.render_active_tab();
        }
    }

    pub fn set_shorts(self: &Rc<Self>, mut paged: PagedVideos) {
        if let Some(ref details) = *self.channel_details.borrow() {
            for short in &mut paged.items {
                if short.channel_id.is_empty() {
                    short.channel_id = details.id.clone();
                }
                if short.channel_name.is_empty() {
                    short.channel_name = details.name.clone();
                }
                if short.channel_avatar_url.is_empty() {
                    short.channel_avatar_url = details.avatar_url.clone();
                }
            }
        }
        *self.shorts_list.borrow_mut() = paged.items;
        *self.shorts_cursor.borrow_mut() = paged.next;
        if self.active_tab.get() == ChannelTab::Shorts && self.search_results.borrow().is_none() {
            self.is_loading.set(false);
            self.render_active_tab();
        }
    }

    pub fn set_load_error(&self, err: &str) {
        self.is_loading.set(false);
        self.stats_label.set_text("Could not load channel details");
        self.show_status("Failed to load channel", err);
    }

    fn reset_spacers_and_window(&self) {
        self.win_start_idx.set(0);
        self.win_end_idx.set(0);
        self.rendered_items.borrow_mut().clear();
        if self.top_spacer.height_request() != 0 {
            self.top_spacer.set_size_request(-1, 0);
        }
        self.top_spacer.set_visible(false);
        if self.bottom_spacer.height_request() != 0 {
            self.bottom_spacer.set_size_request(-1, 0);
        }
        self.bottom_spacer.set_visible(false);
    }

    pub fn suspend_textures(&self) {
        if self.is_suspended.replace(true) {
            return;
        }
        self.saved_scroll_y
            .set(self.scrolled.vadjustment().value().max(0.0));

        let total_items = self.rendered_items.borrow().len();
        if total_items > 0 {
            let is_shorts_tab = self.active_tab.get() == ChannelTab::Shorts
                && self.search_results.borrow().is_none();
            let cols = (if is_shorts_tab {
                self.shorts_cols.get()
            } else {
                self.grid_cols.get()
            })
            .max(1) as usize;
            let total_rows = total_items.div_ceil(cols);
            let stride = if is_shorts_tab {
                self.shorts_row_stride.get()
            } else {
                self.videos_row_stride.get()
            };
            let full_px =
                ((total_rows as f64 * stride) - GRID_ROW_SPACING_PX).max(0.0).round() as i32;
            self.top_spacer.set_size_request(-1, full_px);
            self.top_spacer.set_visible(full_px > 0);
            self.bottom_spacer.set_size_request(-1, 0);
            self.bottom_spacer.set_visible(false);
        }

        if let Some(prev) = self.selected_box.borrow_mut().take() {
            prev.remove_css_class("selected");
        }
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.win_start_idx.set(0);
        self.win_end_idx.set(0);
        self.actions.image_loader.schedule_heap_trim();
    }

    pub fn resume_textures(self: &Rc<Self>) {
        if !self.is_suspended.replace(false) {
            return;
        }
        if self.is_loading.get() {
            return;
        }
        let target_y = self.saved_scroll_y.get();
        if self.active_tab.get() == ChannelTab::Playlists && self.search_results.borrow().is_none()
        {
            self.render_active_tab();
        } else if !self.rendered_items.borrow().is_empty() {
            self.sync_virtual_window(true);
        }
        if target_y > 0.0 {
            self.scrolled.vadjustment().set_value(target_y);
        }
    }

    fn show_skeletons(&self) {
        self.clear_selection();
        self.reset_spacers_and_window();
        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        let cols = self.grid_cols.get();
        self.flow_box.set_homogeneous(true);
        self.flow_box.set_min_children_per_line(cols);
        self.flow_box.set_max_children_per_line(cols);
        for _ in 0..8 {
            self.flow_box.append(&VideoCard::new_skeleton());
        }
    }

    fn show_status(&self, title: &str, description: &str) {
        self.search_more_matches_btn.set_visible(false);
        self.clear_selection();
        self.reset_spacers_and_window();
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.flow_box.set_visible(false);
        self.status_page.set_title(title);
        self.status_page.set_description(Some(description));
        self.status_box.set_visible(true);
    }

    fn filtered_active_items(&self) -> Vec<VideoSummary> {
        let mut base: Vec<VideoSummary> = if let Some(ref search_results) = *self.search_results.borrow() {
            search_results.clone()
        } else {
            match self.active_tab.get() {
                ChannelTab::Videos | ChannelTab::Playlists => self.videos_list.borrow().clone(),
                ChannelTab::Shorts => self.shorts_list.borrow().clone(),
            }
        };

        // Keep YouTube's source order for Shorts Newest/Oldest; their upload timestamps are
        // incomplete. Popular remains locally sorted by view count, which is available on the
        // Shorts cards and was the previously working behavior.
        let has_search_results = self.search_results.borrow().is_some();
        if should_sort_channel_items_locally(
            self.active_tab.get(),
            has_search_results,
            self.sort_order.get(),
        ) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            match self.sort_order.get() {
                ChannelSortOrder::Newest => base.sort_by(|a, b| {
                    compare_optional_timestamps_desc(
                        effective_published_timestamp(a, now),
                        effective_published_timestamp(b, now),
                    )
                }),
                ChannelSortOrder::Oldest => base.sort_by(|a, b| {
                    compare_optional_timestamps(
                        effective_published_timestamp(a, now),
                        effective_published_timestamp(b, now),
                    )
                }),
                ChannelSortOrder::Popular => {
                    base.sort_by(|a, b| compare_view_counts_desc(a.view_count, b.view_count))
                }
            }
        }

        let (df, duf) = if self.active_tab.get() == ChannelTab::Shorts {
            (UploadDateFilter::AnyTime, DurationFilter::AnyLength)
        } else {
            (self.date_filter.get(), self.duration_filter.get())
        };
        base.into_iter()
            .filter(|video| matches_local_filters(video, df, duf))
            .collect()
    }

    pub fn render_active_tab(self: &Rc<Self>) {
        if self.is_loading.get() {
            return;
        }

        self.clear_selection();
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        self.win_start_idx.set(0);
        self.win_end_idx.set(0);

        if self.active_tab.get() == ChannelTab::Playlists && self.search_results.borrow().is_none()
        {
            self.reset_spacers_and_window();
            let pls = self.playlists_list.borrow().clone();
            if pls.is_empty() {
                self.show_status("Nothing here yet", "This channel has no public playlists.");
                return;
            }
            self.status_box.set_visible(false);
            self.flow_box.set_visible(true);
            self.update_grid_columns();
            if !self.is_suspended.get() {
                self.append_playlists_to_flow(&pls);
            }
            return;
        }

        let items = self.filtered_active_items();
        if items.is_empty() {
            let has_channel_filter = has_active_channel_filters(
                self.active_tab.get(),
                self.date_filter.get(),
                self.duration_filter.get(),
            );
            let has_filter = has_channel_filter || self.search_results.borrow().is_some();
            if has_filter {
                let can_scan_more = self.search_results.borrow().is_none()
                    && match self.active_tab.get() {
                        ChannelTab::Shorts => self.shorts_cursor.borrow().is_some(),
                        ChannelTab::Videos => self.videos_cursor.borrow().is_some(),
                        ChannelTab::Playlists => false,
                    };
                if has_channel_filter && can_scan_more && self.filter_scan_pages.get() < 4 {
                    self.filter_scan_pages
                        .set(self.filter_scan_pages.get().saturating_add(1));
                    self.show_status(
                        "Looking for matching videos",
                        "Loading another page of channel videos…",
                    );
                    self.load_more_active_tab();
                    return;
                }
                if has_channel_filter && can_scan_more {
                    self.show_status(
                        "No matches in the loaded videos",
                        "You can check more of the channel or clear the filters.",
                    );
                    self.search_more_matches_btn.set_visible(true);
                } else {
                    self.show_status(
                        "No matching videos",
                        "Try clearing your channel search or resetting filters.",
                    );
                }
            } else {
                let label = match self.active_tab.get() {
                    ChannelTab::Videos => "This channel has no videos yet.",
                    ChannelTab::Shorts => "This channel has no Shorts yet.",
                    ChannelTab::Playlists => "This channel has no public playlists.",
                };
                self.show_status("Nothing here yet", label);
            }
            return;
        }

        self.status_box.set_visible(false);
        self.flow_box.set_visible(true);
        self.update_grid_columns();
        *self.rendered_items.borrow_mut() = items;

        if !self.is_suspended.get() {
            self.sync_virtual_window(true);
        }
        self.actions.image_loader.schedule_heap_trim();

        // Keep scanning filtered pages until the grid has a useful batch of matches, but cap
        // automatic requests so a restrictive filter cannot silently crawl an entire channel.
        let has_channel_filter = has_active_channel_filters(
            self.active_tab.get(),
            self.date_filter.get(),
            self.duration_filter.get(),
        );
        let has_more = match self.active_tab.get() {
            ChannelTab::Videos => self.videos_cursor.borrow().is_some(),
            ChannelTab::Shorts => self.shorts_cursor.borrow().is_some(),
            ChannelTab::Playlists => false,
        };
        if has_channel_filter
            && self.search_results.borrow().is_none()
            && self.rendered_items.borrow().len() < 12
            && self.filter_scan_pages.get() < 4
            && has_more
        {
            self.filter_scan_pages
                .set(self.filter_scan_pages.get().saturating_add(1));
            self.load_more_active_tab();
        }
    }

    fn on_scroll_changed(self: &Rc<Self>) {
        if self.is_loading.get() || self.is_suspended.get() || self.in_window_sync.get() {
            return;
        }
        if self.active_tab.get() == ChannelTab::Playlists && self.search_results.borrow().is_none()
        {
            return;
        }
        if !self.rendered_items.borrow().is_empty() {
            self.sync_virtual_window(false);
        }
    }

    fn sync_virtual_window(self: &Rc<Self>, force_rebuild: bool) {
        if self.in_window_sync.get() || self.is_loading.get() || self.is_suspended.get() {
            return;
        }
        if self.active_tab.get() == ChannelTab::Playlists && self.search_results.borrow().is_none()
        {
            return;
        }
        let total_items = self.rendered_items.borrow().len();
        if total_items == 0 {
            return;
        }

        let is_shorts_tab =
            self.active_tab.get() == ChannelTab::Shorts && self.search_results.borrow().is_none();
        let cols = (if is_shorts_tab {
            self.shorts_cols.get()
        } else {
            self.grid_cols.get()
        })
        .max(1) as usize;

        let cur_start = self.win_start_idx.get();
        let cur_end = self.win_end_idx.get();
        let cur_mounted_rows = if cur_end > cur_start {
            (cur_end - cur_start).div_ceil(cols)
        } else {
            0
        };

        if !force_rebuild && cur_mounted_rows >= 2 {
            let fb_h = self.flow_box.height();
            if fb_h > 120 {
                let measured = (fb_h as f64 + GRID_ROW_SPACING_PX) / (cur_mounted_rows as f64);
                if (160.0..=620.0).contains(&measured) {
                    let prev = if is_shorts_tab {
                        self.shorts_row_stride.get()
                    } else {
                        self.videos_row_stride.get()
                    };
                    if (measured - prev).abs() >= 0.5 {
                        if is_shorts_tab {
                            self.shorts_row_stride.set(measured);
                        } else {
                            self.videos_row_stride.set(measured);
                        }
                    }
                }
            }
        }

        let stride = if is_shorts_tab {
            self.shorts_row_stride.get()
        } else {
            self.videos_row_stride.get()
        };

        let scroll_y = self.scrolled.vadjustment().value().max(0.0);
        let (new_start, new_end, top_px, bottom_px) = compute_virtual_slice(
            total_items,
            cols,
            stride,
            scroll_y,
            cur_start,
            cur_end,
            force_rebuild,
        );

        self.in_window_sync.set(true);

        let mut removed_any = false;
        if force_rebuild || cur_end <= cur_start || new_end <= cur_start || new_start >= cur_end {
            while let Some(child) = self.flow_box.first_child() {
                self.flow_box.remove(&child);
                removed_any = true;
            }
            let slice: Vec<VideoSummary> =
                self.rendered_items.borrow()[new_start..new_end].to_vec();
            for (off, item) in slice.iter().enumerate() {
                let card = self.build_single_item_card(item, is_shorts_tab, new_start + off);
                self.flow_box.append(&card.container);
            }
        } else if new_start != cur_start || new_end != cur_end {
            if new_start > cur_start {
                for _ in 0..(new_start - cur_start) {
                    if let Some(child) = self.flow_box.first_child() {
                        self.flow_box.remove(&child);
                        removed_any = true;
                    }
                }
            }
            if new_end < cur_end {
                for _ in 0..(cur_end - new_end) {
                    if let Some(child) = self.flow_box.last_child() {
                        self.flow_box.remove(&child);
                        removed_any = true;
                    }
                }
            }
            if new_start < cur_start {
                let prepend_slice: Vec<VideoSummary> =
                    self.rendered_items.borrow()[new_start..cur_start].to_vec();
                for (off, item) in prepend_slice.iter().enumerate().rev() {
                    let card = self.build_single_item_card(item, is_shorts_tab, new_start + off);
                    self.flow_box.prepend(&card.container);
                }
            }
            if new_end > cur_end {
                let append_slice: Vec<VideoSummary> =
                    self.rendered_items.borrow()[cur_end..new_end].to_vec();
                for (off, item) in append_slice.iter().enumerate() {
                    let card = self.build_single_item_card(item, is_shorts_tab, cur_end + off);
                    self.flow_box.append(&card.container);
                }
            }
        }

        self.win_start_idx.set(new_start);
        self.win_end_idx.set(new_end);

        if self.top_spacer.height_request() != top_px {
            self.top_spacer.set_size_request(-1, top_px);
        }
        self.top_spacer.set_visible(top_px > 0);

        if self.bottom_spacer.height_request() != bottom_px {
            self.bottom_spacer.set_size_request(-1, bottom_px);
        }
        self.bottom_spacer.set_visible(bottom_px > 0);

        self.in_window_sync.set(false);

        if removed_any {
            self.actions.image_loader.schedule_heap_trim();
        }
    }

    fn append_playlists_to_flow(self: &Rc<Self>, pls: &[PlaylistSearchSummary]) {
        let layout = if self.is_narrow.get() {
            ListCardLayout::Stacked
        } else if self.is_compact.get() {
            ListCardLayout::Compact
        } else {
            ListCardLayout::Wide
        };

        for pl in pls {
            let initially_saved = self
                .actions
                .playlist_store
                .find_by_youtube_id(&pl.id)
                .is_some();
            let pid_open = pl.id.clone();
            let open_cb = self.on_open_playlist.clone();
            let open_action: Rc<dyn Fn()> = Rc::new(move || open_cb(pid_open.clone()));

            let cp_sel = Rc::downgrade(self);
            let pl_id_sel = pl.id.clone();
            let enter_cb = open_action.clone();
            let open_dbl = open_action.clone();

            let pid_save = pl.id.clone();
            let pname_save = pl.name.clone();
            let pchan_save = pl.channel_name.clone();
            let cp_save = Rc::downgrade(self);

            let card = PlaylistResultCard::new(
                pl,
                layout,
                initially_saved,
                move |hl| {
                    if let Some(cp) = cp_sel.upgrade() {
                        cp.select_card_box(Some(&pl_id_sel), hl, enter_cb.clone());
                    }
                },
                move || open_dbl(),
                move || {
                    let Some(cp) = cp_save.upgrade() else {
                        return;
                    };
                    if cp
                        .actions
                        .playlist_store
                        .find_by_youtube_id(&pid_save)
                        .is_some()
                    {
                        (cp.actions.on_toast)("Playlist already saved".to_string());
                        return;
                    }
                    let engine = cp.engine.clone();
                    let pid = pid_save.clone();
                    let pname = pname_save.clone();
                    let pchan = pchan_save.clone();
                    let pstore = cp.actions.playlist_store.clone();
                    let on_toast = cp.actions.on_toast.clone();
                    let on_pl_changed = cp.actions.on_playlists_changed.clone();
                    let (tx, rx) = async_channel::bounded::<
                        Result<crate::backend::engine::RemotePlaylist, String>,
                    >(1);
                    cp.runtime.spawn(async move {
                        let res = engine.fetch_playlist(&pid).await;
                        let _ = tx.send(res).await;
                    });
                    glib::spawn_future_local(async move {
                        if let Ok(Ok(remote)) = rx.recv().await {
                            pstore.create_with_videos(
                                &pname,
                                remote.videos,
                                Some(remote.id),
                                &pchan,
                            );
                            on_pl_changed();
                            on_toast(format!("Saved playlist “{pname}”"));
                        }
                    });
                },
            );

            self.actions.image_loader.load_into_picture(
                &format!("pl_{}", pl.id),
                &pl.thumbnail_url,
                &card.picture,
            );
            self.flow_box.append(&card.container);
        }
    }

    fn build_single_item_card(
        self: &Rc<Self>,
        item: &VideoSummary,
        is_shorts_tab: bool,
        idx: usize,
    ) -> VideoCard {
        if is_shorts_tab {
            let card = VideoCard::new_short(item);
            self.actions.image_loader.load_into_short_picture(
                &item.id,
                &item.thumbnail_url,
                &card.picture,
            );

            let short_id = item.id.clone();
            let short_id_sel = item.id.clone();
            let short_id_det = item.id.clone();
            let cp_weak_play = Rc::downgrade(self);
            let play_cb: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(cp) = cp_weak_play.upgrade() {
                    let current_all = cp.filtered_active_items();
                    if !current_all.is_empty() {
                        let target_idx = current_all
                            .iter()
                            .position(|v| v.id == short_id)
                            .unwrap_or(idx);
                        let cont = cp.shorts_continuation_payload();
                        (cp.on_play_shorts_queue)(current_all, target_idx, None, cont);
                    }
                }
            });

            let cp_weak_sel = Rc::downgrade(self);
            let cp_weak_det = Rc::downgrade(self);
            let play_for_enter = play_cb.clone();
            let play_for_dbl = play_cb;
            let parent_weak = self.parent_window.clone();
            let sum_det = item.clone();
            let mut act_det = self.actions.clone();
            act_det.on_play = Rc::new(move |_summary, q| {
                if let Some(cp) = cp_weak_det.upgrade() {
                    let current_all = cp.filtered_active_items();
                    if !current_all.is_empty() {
                        let target_idx = current_all
                            .iter()
                            .position(|v| v.id == short_id_det)
                            .unwrap_or(idx);
                        let cont = cp.shorts_continuation_payload();
                        (cp.on_play_shorts_queue)(current_all, target_idx, Some(q), cont);
                    }
                }
            });

            if self.selected_item_id.borrow().as_deref() == Some(item.id.as_str()) {
                card.highlight_target.add_css_class("selected");
                *self.selected_box.borrow_mut() = Some(card.highlight_target.clone());
            }

            card.connect_card_interactions(
                move |hl| {
                    if let Some(cp) = cp_weak_sel.upgrade() {
                        cp.select_card_box(Some(&short_id_sel), hl, play_for_enter.clone());
                    }
                },
                move || play_for_dbl(),
                move || {
                    if let Some(win) = parent_weak.upgrade() {
                        DetailsSheet::show(&win, &sum_det, &act_det);
                    }
                },
            );

            card
        } else {
            let card = VideoCard::new_grid(item);
            self.actions.image_loader.load_into_picture(
                &item.id,
                &item.thumbnail_url,
                &card.picture,
            );
            self.actions.image_loader.load_into_avatar(
                &item.channel_id,
                &item.channel_avatar_url,
                &card.avatar,
            );

            let sum_play = item.clone();
            let vid_id_sel = item.id.clone();
            let on_play = self.actions.on_play.clone();
            let def_q = self.actions.default_quality.clone();
            let play_cb: Rc<dyn Fn()> = Rc::new(move || {
                on_play(sum_play.clone(), def_q());
            });

            let cp_weak_sel = Rc::downgrade(self);
            let play_for_enter = play_cb.clone();
            let play_for_dbl = play_cb;
            let parent_weak = self.parent_window.clone();
            let sum_det = item.clone();
            let act_det = self.actions.clone();

            if self.selected_item_id.borrow().as_deref() == Some(item.id.as_str()) {
                card.highlight_target.add_css_class("selected");
                *self.selected_box.borrow_mut() = Some(card.highlight_target.clone());
            }

            card.connect_card_interactions(
                move |hl| {
                    if let Some(cp) = cp_weak_sel.upgrade() {
                        cp.select_card_box(Some(&vid_id_sel), hl, play_for_enter.clone());
                    }
                },
                move || play_for_dbl(),
                move || {
                    if let Some(win) = parent_weak.upgrade() {
                        DetailsSheet::show(&win, &sum_det, &act_det);
                    }
                },
            );

            card
        }
    }
}

fn should_sort_channel_items_locally(
    tab: ChannelTab,
    has_search_results: bool,
    order: ChannelSortOrder,
) -> bool {
    has_search_results || tab != ChannelTab::Shorts || order == ChannelSortOrder::Popular
}

fn has_active_channel_filters(
    tab: ChannelTab,
    date_filter: UploadDateFilter,
    duration_filter: DurationFilter,
) -> bool {
    tab != ChannelTab::Shorts
        && (date_filter != UploadDateFilter::AnyTime
            || duration_filter != DurationFilter::AnyLength)
}

fn compare_view_counts_desc(left: Option<u64>, right: Option<u64>) -> std::cmp::Ordering {
    right.cmp(&left)
}

fn compare_optional_timestamps(left: Option<i64>, right: Option<i64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn compare_optional_timestamps_desc(left: Option<i64>, right: Option<i64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn create_pop_item(label: &str) -> Button {
    let btn = Button::builder().label(label).halign(Align::Fill).build();
    btn.add_css_class("flat");
    btn.add_css_class("yr-menu-item");
    set_pointer(&btn);
    if let Some(child) = btn.child() {
        if let Some(lbl) = child.downcast_ref::<Label>() {
            lbl.set_halign(Align::Start);
            lbl.set_xalign(0.0);
        }
    }
    btn
}

fn set_pop_item_text(btn: &Button, label: &str, selected: bool) {
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

fn compute_virtual_slice(
    total_items: usize,
    cols: usize,
    stride: f64,
    scroll_y: f64,
    cur_start: usize,
    cur_end: usize,
    force_rebuild: bool,
) -> (usize, usize, i32, i32) {
    let cols = cols.max(1);
    if total_items == 0 {
        return (0, 0, 0, 0);
    }
    let total_rows = total_items.div_ceil(cols);
    let approx_first_row = (scroll_y.max(0.0) / stride.max(1.0)).floor() as usize;
    let max_start_row = total_rows.saturating_sub(VIRTUAL_WINDOW_ROWS);
    let ideal_start_row = approx_first_row.saturating_sub(2).min(max_start_row);

    let cur_start_row = cur_start / cols;
    let target_start_row = if force_rebuild || cur_end <= cur_start {
        ideal_start_row
    } else if ideal_start_row == 0 || ideal_start_row == max_start_row {
        ideal_start_row
    } else if ideal_start_row.abs_diff(cur_start_row) >= VIRTUAL_HYSTERESIS_ROWS {
        ideal_start_row
    } else {
        cur_start_row.min(max_start_row)
    };

    let new_start = (target_start_row * cols).min(total_items);
    let new_end = ((target_start_row + VIRTUAL_WINDOW_ROWS) * cols).min(total_items);

    let new_start_row = new_start / cols;
    let new_end_row = new_end.div_ceil(cols);
    let rem_rows = total_rows.saturating_sub(new_end_row);
    let top_px = (new_start_row as f64 * stride).round() as i32;
    let bottom_px = (rem_rows as f64 * stride).round() as i32;

    (new_start, new_end, top_px, bottom_px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_shorts_only_locally_sort_popular() {
        assert!(!should_sort_channel_items_locally(
            ChannelTab::Shorts,
            false,
            ChannelSortOrder::Newest
        ));
        assert!(!should_sort_channel_items_locally(
            ChannelTab::Shorts,
            false,
            ChannelSortOrder::Oldest
        ));
        assert!(should_sort_channel_items_locally(
            ChannelTab::Shorts,
            false,
            ChannelSortOrder::Popular
        ));
        assert!(should_sort_channel_items_locally(
            ChannelTab::Videos,
            false,
            ChannelSortOrder::Newest
        ));
        assert!(should_sort_channel_items_locally(
            ChannelTab::Shorts,
            true,
            ChannelSortOrder::Newest
        ));
    }

    #[test]
    fn shorts_listing_ignores_video_filter_state() {
        assert!(!has_active_channel_filters(
            ChannelTab::Shorts,
            UploadDateFilter::ThisMonth,
            DurationFilter::Under4Min
        ));
        assert!(has_active_channel_filters(
            ChannelTab::Videos,
            UploadDateFilter::ThisMonth,
            DurationFilter::AnyLength
        ));
        assert!(!has_active_channel_filters(
            ChannelTab::Shorts,
            UploadDateFilter::ThisMonth,
            DurationFilter::AnyLength
        ));
    }

    #[test]
    fn popular_sort_uses_descending_views_and_keeps_unknown_counts_last() {
        assert_eq!(
            compare_view_counts_desc(Some(100), Some(20)),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_view_counts_desc(Some(20), None),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn sort_timestamp_comparator_orders_newest_oldest_and_unknowns_last() {
        assert_eq!(
            compare_optional_timestamps(Some(30), Some(20)),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_optional_timestamps(Some(20), Some(30)),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_optional_timestamps(Some(20), None),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_optional_timestamps_desc(Some(30), Some(20)),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_optional_timestamps_desc(Some(20), None),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_virtual_slice_bounds_live_widgets_and_preserves_total_height() {
        // 600 Shorts across 6 columns = 100 rows, stride = 366px (342px card + 24px gap)
        let total_items = 600;
        let cols = 6;
        let stride = 366.0;
        let expected_total_h = (100 * 366) - 24;

        let mut cur_start = 0;
        let mut cur_end = 0;

        for row in 0..95 {
            let scroll_y = row as f64 * stride;
            let (s, e, top_px, bottom_px) = compute_virtual_slice(
                total_items,
                cols,
                stride,
                scroll_y,
                cur_start,
                cur_end,
                cur_end == 0,
            );
            cur_start = s;
            cur_end = e;

            // Never mounts more than 8 rows (48 Short cards) at once
            assert!(e - s <= VIRTUAL_WINDOW_ROWS * cols);
            assert_eq!(s % cols, 0);

            // Exact pixel height invariant: top_spacer + flow_box + bottom_spacer == total_height
            let mounted_rows = (e - s).div_ceil(cols) as i32;
            let flow_px = mounted_rows * 366 - 24;
            assert_eq!(top_px + flow_px + bottom_px, expected_total_h);
        }

        // Scrolling back to y = 0 snaps cleanly to index 0 with 0px top spacer
        let (s0, e0, top0, _) =
            compute_virtual_slice(total_items, cols, stride, 0.0, cur_start, cur_end, false);
        assert_eq!(s0, 0);
        assert_eq!(e0, 48);
        assert_eq!(top0, 0);
    }
}
