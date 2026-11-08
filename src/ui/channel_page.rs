use crate::backend::models::{ChannelDetails, VideoSummary};
use crate::backend::subscriptions::{SubscribedChannel, SubscriptionStore};
use crate::ui::video_card::VideoCard;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, FlowBox, Label, Orientation, Popover,
    ScrolledWindow, SearchEntry, SelectionMode, Separator, ToggleButton,
};
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChannelTab {
    Videos,
    Shorts,
}

pub struct ChannelPage {
    pub page: adw::NavigationPage,
    avatar: adw::Avatar,
    name_label: Label,
    stats_label: Label,
    sub_btn: Button,
    tab_videos: ToggleButton,
    tab_shorts: ToggleButton,
    play_all_btn: Button,
    search_entry: SearchEntry,
    filter_btn: Button,
    sort_btn: Button,
    flow_box: FlowBox,
    active_tab: RefCell<ChannelTab>,
    channel_details: RefCell<Option<ChannelDetails>>,
    videos_list: RefCell<Vec<VideoSummary>>,
    shorts_list: RefCell<Vec<VideoSummary>>,
    store: SubscriptionStore,
}

impl ChannelPage {
    pub fn new(channel_id: &str, channel_name: &str) -> Rc<Self> {
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

        let stats_label = Label::new(Some("Loading channel stats..."));
        stats_label.add_css_class("yr-meta");
        stats_label.set_halign(Align::Start);
        stats_label.set_xalign(0.0);

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
        let toolbar_box = GtkBox::new(Orientation::Horizontal, 12);
        toolbar_box.set_margin_start(28);
        toolbar_box.set_margin_end(28);
        toolbar_box.set_margin_top(4);
        toolbar_box.set_margin_bottom(16);
        toolbar_box.set_valign(Align::Center);

        // Segmented Videos | Shorts
        let seg_box = GtkBox::new(Orientation::Horizontal, 2);
        seg_box.add_css_class("yr-segmented");

        let tab_videos = ToggleButton::with_label("Videos");
        tab_videos.add_css_class("yr-chip");
        tab_videos.set_active(true);

        let tab_shorts = ToggleButton::with_label("Shorts");
        tab_shorts.add_css_class("yr-chip");
        tab_shorts.set_group(Some(&tab_videos));

        seg_box.append(&tab_videos);
        seg_box.append(&tab_shorts);
        toolbar_box.append(&seg_box);

        // Play All (for Shorts)
        let play_all_btn = Button::builder()
            .label("Play all")
            .icon_name("media-playback-start-symbolic")
            .build();
        play_all_btn.add_css_class("yr-btn");
        play_all_btn.add_css_class("primary");
        play_all_btn.set_visible(false); // only in Shorts tab
        toolbar_box.append(&play_all_btn);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar_box.append(&spacer);

        // Search in Channel
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search in channel")
            .width_request(220)
            .build();
        search_entry.add_css_class("yr-search");
        toolbar_box.append(&search_entry);

        // Filter Button + Popover
        let filter_btn = Button::with_label("Filter");
        filter_btn.add_css_class("yr-chip");
        setup_filter_popover(&filter_btn);
        toolbar_box.append(&filter_btn);

        // Sort Button + Popover
        let sort_btn = Button::with_label("Sort: Newest");
        sort_btn.add_css_class("yr-chip");
        setup_sort_popover(&sort_btn);
        toolbar_box.append(&sort_btn);

        root_box.append(&toolbar_box);

        // --- Video / Shorts Grid ---
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

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&flow_box)
            .vexpand(true)
            .hexpand(true)
            .build();

        root_box.append(&scrolled);

        let page = adw::NavigationPage::builder()
            .title(channel_name)
            .tag(channel_id)
            .child(&root_box)
            .build();

        let store = SubscriptionStore::new();
        let is_sub = store.is_subscribed(channel_id);
        if is_sub {
            sub_btn.set_label("Subscribed");
            sub_btn.remove_css_class("primary");
        }

        let this = Rc::new(Self {
            page,
            avatar,
            name_label,
            stats_label,
            sub_btn,
            tab_videos,
            tab_shorts,
            play_all_btn,
            search_entry,
            filter_btn,
            sort_btn,
            flow_box,
            active_tab: RefCell::new(ChannelTab::Videos),
            channel_details: RefCell::new(None),
            videos_list: RefCell::new(Vec::new()),
            shorts_list: RefCell::new(Vec::new()),
            store,
        });

        // Tab switching
        let this_tab_v = this.clone();
        this.tab_videos.connect_toggled(move |btn| {
            if btn.is_active() {
                *this_tab_v.active_tab.borrow_mut() = ChannelTab::Videos;
                this_tab_v.play_all_btn.set_visible(false);
                this_tab_v.flow_box.set_max_children_per_line(4);
                this_tab_v.flow_box.set_column_spacing(20);
                this_tab_v.render_active_tab();
            }
        });

        let this_tab_s = this.clone();
        this.tab_shorts.connect_toggled(move |btn| {
            if btn.is_active() {
                *this_tab_s.active_tab.borrow_mut() = ChannelTab::Shorts;
                this_tab_s.play_all_btn.set_visible(true);
                this_tab_s.flow_box.set_max_children_per_line(6);
                this_tab_s.flow_box.set_column_spacing(14);
                this_tab_s.render_active_tab();
            }
        });

        // Subscribe button toggle
        let this_sub = this.clone();
        let cid = channel_id.to_string();
        this.sub_btn.connect_clicked(move |btn| {
            let is_sub = this_sub.store.is_subscribed(&cid);
            if is_sub {
                this_sub.store.unsubscribe(&cid);
                btn.set_label("Subscribe");
                btn.add_css_class("primary");
            } else {
                let det = this_sub.channel_details.borrow();
                let sub_item = SubscribedChannel {
                    id: cid.clone(),
                    name: det.as_ref().map(|d| d.name.clone()).unwrap_or_default(),
                    handle: det.as_ref().map(|d| d.handle.clone()).unwrap_or_default(),
                    avatar_url: det.as_ref().map(|d| d.avatar_url.clone()).unwrap_or_default(),
                };
                this_sub.store.subscribe(sub_item);
                btn.set_label("Subscribed");
                btn.remove_css_class("primary");
            }
        });

        this
    }

    pub fn set_channel_details(&self, details: ChannelDetails) {
        self.name_label.set_text(&details.name);
        self.avatar.set_text(Some(&details.name));
        self.stats_label.set_text(&format!(
            "{} · {} · {}",
            details.handle, details.subscriber_count_text, details.video_count_text
        ));
        *self.channel_details.borrow_mut() = Some(details);
    }

    pub fn switch_tab(&self, tab: ChannelTab) {
        match tab {
            ChannelTab::Videos => self.tab_videos.set_active(true),
            ChannelTab::Shorts => self.tab_shorts.set_active(true),
        }
    }

    pub fn set_videos(&self, videos: Vec<VideoSummary>) {
        *self.videos_list.borrow_mut() = videos;
        if *self.active_tab.borrow() == ChannelTab::Videos {
            self.render_active_tab();
        }
    }

    pub fn set_shorts(&self, shorts: Vec<VideoSummary>) {
        *self.shorts_list.borrow_mut() = shorts;
        if *self.active_tab.borrow() == ChannelTab::Shorts {
            self.render_active_tab();
        }
    }

    pub fn connect_play_all(&self, callback: impl Fn(Vec<VideoSummary>) + 'static) {
        let shorts = self.shorts_list.borrow().clone();
        self.play_all_btn.connect_clicked(move |_| {
            callback(shorts.clone());
        });
    }

    pub fn render_active_tab(&self) {
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }

        let tab = *self.active_tab.borrow();
        match tab {
            ChannelTab::Videos => {
                for video in self.videos_list.borrow().iter() {
                    let card = VideoCard::new_grid(video);
                    self.flow_box.append(&card.container);
                }
            }
            ChannelTab::Shorts => {
                for short in self.shorts_list.borrow().iter() {
                    let card = VideoCard::new_short(short);
                    self.flow_box.append(&card.container);
                }
            }
        }
    }
}

fn setup_filter_popover(btn: &Button) {
    let popover = Popover::new();
    popover.set_parent(btn);
    popover.set_has_arrow(false);

    let content = GtkBox::new(Orientation::Vertical, 4);
    content.set_width_request(200);

    let h1 = Label::new(Some("UPLOAD DATE"));
    h1.add_css_class("yr-popover-head");
    h1.set_halign(Align::Start);
    content.append(&h1);

    content.append(&create_pop_item("Any time ✓"));
    content.append(&create_pop_item("Today"));
    content.append(&create_pop_item("This week"));
    content.append(&create_pop_item("This month"));
    content.append(&create_pop_item("This year"));

    let sep = Separator::new(Orientation::Horizontal);
    sep.set_margin_top(4);
    sep.set_margin_bottom(4);
    content.append(&sep);

    let h2 = Label::new(Some("DURATION"));
    h2.add_css_class("yr-popover-head");
    h2.set_halign(Align::Start);
    content.append(&h2);

    content.append(&create_pop_item("Any length ✓"));
    content.append(&create_pop_item("Under 4 min"));
    content.append(&create_pop_item("4 to 20 min"));
    content.append(&create_pop_item("Over 20 min"));

    let footer = GtkBox::new(Orientation::Horizontal, 8);
    footer.set_margin_top(8);
    let reset_btn = Button::with_label("Reset");
    reset_btn.add_css_class("yr-chip");
    let apply_btn = Button::with_label("Apply");
    apply_btn.add_css_class("yr-chip");
    apply_btn.add_css_class("active");
    footer.append(&reset_btn);
    footer.append(&apply_btn);
    content.append(&footer);

    popover.set_child(Some(&content));

    let pop_weak = popover.downgrade();
    btn.connect_clicked(move |_| {
        if let Some(pop) = pop_weak.upgrade() {
            pop.popup();
        }
    });
}

fn setup_sort_popover(btn: &Button) {
    let popover = Popover::new();
    popover.set_parent(btn);
    popover.set_has_arrow(false);

    let content = GtkBox::new(Orientation::Vertical, 4);
    content.set_width_request(180);

    let h = Label::new(Some("SORT BY"));
    h.add_css_class("yr-popover-head");
    h.set_halign(Align::Start);
    content.append(&h);

    content.append(&create_pop_item("Newest ✓"));
    content.append(&create_pop_item("Oldest"));
    content.append(&create_pop_item("Most popular"));

    popover.set_child(Some(&content));

    let pop_weak = popover.downgrade();
    btn.connect_clicked(move |_| {
        if let Some(pop) = pop_weak.upgrade() {
            pop.popup();
        }
    });
}

fn create_pop_item(label: &str) -> Button {
    let btn = Button::builder().label(label).halign(Align::Fill).build();
    btn.add_css_class("flat");
    btn.add_css_class("yr-menu-item");
    if let Some(child) = btn.child() {
        if let Some(lbl) = child.downcast_ref::<Label>() {
            lbl.set_halign(Align::Start);
            lbl.set_xalign(0.0);
        }
    }
    btn
}
