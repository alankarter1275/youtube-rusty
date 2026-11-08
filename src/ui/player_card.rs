use crate::backend::models::VideoSummary;
use gtk4::gdk::Texture;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, Overlay, Picture, ProgressBar,
    ScrolledWindow, Separator,
};
use std::cell::RefCell;
use std::rc::Rc;

pub struct PlayerCard {
    pub root: GtkBox,
    thumb_pic: Picture,
    title_label: Label,
    channel_btn: Button,
    queue_pos_label: Label,
    progress_bar: ProgressBar,
    time_elapsed_label: Label,
    time_total_label: Label,
    pub prev_btn: Button,
    pub play_pause_btn: Button,
    pub next_btn: Button,
    pub close_btn: Button,
    pub expand_btn: Button,
    queue_box: GtkBox,
    queue_list: GtkBox,
    queue_summary_label: Label,
    shuffle_btn: Button,
    clear_btn: Button,
    is_expanded: RefCell<bool>,
    is_playing: RefCell<bool>,
    queue: RefCell<Vec<VideoSummary>>,
    current_index: RefCell<usize>,
}

impl PlayerCard {
    pub fn new() -> Rc<Self> {
        let root = GtkBox::new(Orientation::Vertical, 10);
        root.set_width_request(370);
        root.set_halign(Align::End);
        root.set_valign(Align::End);
        root.add_css_class("yr-player");
        root.set_visible(false);

        // --- Collapsed Top Row ---
        let top_row = GtkBox::new(Orientation::Horizontal, 12);
        top_row.set_valign(Align::Center);

        let thumb_pic = Picture::new();
        thumb_pic.set_width_request(96);
        thumb_pic.set_height_request(54);
        thumb_pic.set_content_fit(gtk4::ContentFit::Cover);

        let thumb_overlay = Overlay::new();
        thumb_overlay.set_child(Some(&thumb_pic));
        thumb_overlay.add_css_class("yr-thumb");

        let text_col = GtkBox::new(Orientation::Vertical, 2);
        text_col.set_hexpand(true);
        text_col.set_valign(Align::Center);

        let title_label = Label::new(Some("No video playing"));
        title_label.add_css_class("yr-title");
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_lines(1);

        let channel_btn = Button::builder().label("Channel").build();
        channel_btn.add_css_class("flat");
        channel_btn.add_css_class("yr-link");
        channel_btn.set_halign(Align::Start);

        let queue_pos_label = Label::new(Some("Queue · 0 of 0"));
        queue_pos_label.add_css_class("yr-meta");
        queue_pos_label.set_halign(Align::Start);
        queue_pos_label.set_xalign(0.0);

        text_col.append(&title_label);
        text_col.append(&channel_btn);
        text_col.append(&queue_pos_label);

        let close_btn = Button::builder()
            .icon_name("window-close-symbolic")
            .valign(Align::Start)
            .build();
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");

        top_row.append(&thumb_overlay);
        top_row.append(&text_col);
        top_row.append(&close_btn);
        root.append(&top_row);

        // --- Progress Bar & Times ---
        let progress_bar = ProgressBar::new();
        progress_bar.add_css_class("yr-progress");
        progress_bar.set_fraction(0.0);
        root.append(&progress_bar);

        let times_row = GtkBox::new(Orientation::Horizontal, 0);
        let time_elapsed_label = Label::new(Some("0:00"));
        time_elapsed_label.add_css_class("yr-meta");
        time_elapsed_label.set_halign(Align::Start);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);

        let time_total_label = Label::new(Some("0:00"));
        time_total_label.add_css_class("yr-meta");
        time_total_label.set_halign(Align::End);

        times_row.append(&time_elapsed_label);
        times_row.append(&spacer);
        times_row.append(&time_total_label);
        root.append(&times_row);

        // --- Controls Row ---
        let controls_row = GtkBox::new(Orientation::Horizontal, 8);
        controls_row.set_valign(Align::Center);

        let prev_btn = Button::builder()
            .icon_name("media-skip-backward-symbolic")
            .build();
        prev_btn.add_css_class("flat");

        let play_pause_btn = Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .build();
        play_pause_btn.add_css_class("yr-rail-btn");
        play_pause_btn.add_css_class("active"); // Terracotta accent circle!

        let next_btn = Button::builder()
            .icon_name("media-skip-forward-symbolic")
            .build();
        next_btn.add_css_class("flat");

        let ctrl_spacer = GtkBox::new(Orientation::Horizontal, 0);
        ctrl_spacer.set_hexpand(true);

        let expand_btn = Button::builder().label("Queue · 0 ^").build();
        expand_btn.add_css_class("yr-chip");

        controls_row.append(&prev_btn);
        controls_row.append(&play_pause_btn);
        controls_row.append(&next_btn);
        controls_row.append(&ctrl_spacer);
        controls_row.append(&expand_btn);
        root.append(&controls_row);

        // --- Expanded Queue Section ---
        let queue_box = GtkBox::new(Orientation::Vertical, 8);
        queue_box.set_visible(false);

        let divider = Separator::new(Orientation::Horizontal);
        divider.set_margin_top(4);
        divider.set_margin_bottom(4);
        queue_box.append(&divider);

        let queue_header = GtkBox::new(Orientation::Horizontal, 8);
        queue_header.set_valign(Align::Center);

        let queue_summary_label = Label::new(Some("0 videos · drag to reorder"));
        queue_summary_label.add_css_class("yr-meta");
        queue_summary_label.set_halign(Align::Start);

        let q_head_spacer = GtkBox::new(Orientation::Horizontal, 0);
        q_head_spacer.set_hexpand(true);

        let shuffle_btn = Button::with_label("Shuffle");
        shuffle_btn.add_css_class("yr-chip");

        let clear_btn = Button::with_label("Clear");
        clear_btn.add_css_class("yr-chip");

        queue_header.append(&queue_summary_label);
        queue_header.append(&q_head_spacer);
        queue_header.append(&shuffle_btn);
        queue_header.append(&clear_btn);
        queue_box.append(&queue_header);

        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .max_content_height(220)
            .propagate_natural_height(true)
            .build();

        let queue_list = GtkBox::new(Orientation::Vertical, 4);
        scrolled.set_child(Some(&queue_list));
        queue_box.append(&scrolled);
        root.append(&queue_box);

        let card = Rc::new(Self {
            root,
            thumb_pic,
            title_label,
            channel_btn,
            queue_pos_label,
            progress_bar,
            time_elapsed_label,
            time_total_label,
            prev_btn,
            play_pause_btn,
            next_btn,
            close_btn,
            expand_btn,
            queue_box,
            queue_list,
            queue_summary_label,
            shuffle_btn,
            clear_btn,
            is_expanded: RefCell::new(false),
            is_playing: RefCell::new(true),
            queue: RefCell::new(Vec::new()),
            current_index: RefCell::new(0),
        });

        // Setup expand toggle
        let card_weak = Rc::downgrade(&card);
        card.expand_btn.connect_clicked(move |_| {
            if let Some(c) = card_weak.upgrade() {
                let mut exp = c.is_expanded.borrow_mut();
                *exp = !*exp;
                let is_exp = *exp;
                c.queue_box.set_visible(is_exp);
                let q_count = c.queue.borrow().len();
                if is_exp {
                    c.expand_btn.set_label(&format!("Queue · {q_count} ⌵"));
                } else {
                    c.expand_btn.set_label(&format!("Queue · {q_count} ⌃"));
                }
            }
        });

        card
    }

    pub fn set_queue(&self, items: Vec<VideoSummary>, current_pos: usize) {
        *self.queue.borrow_mut() = items;
        *self.current_index.borrow_mut() = current_pos;
        self.update_ui();
    }

    pub fn add_item(&self, item: VideoSummary) {
        self.queue.borrow_mut().push(item);
        self.update_ui();
    }

    pub fn set_current_index(&self, index: usize) {
        *self.current_index.borrow_mut() = index;
        self.update_ui();
    }

    pub fn set_progress(&self, elapsed: f64, duration: f64) {
        if duration > 0.0 {
            let frac = (elapsed / duration).clamp(0.0, 1.0);
            self.progress_bar.set_fraction(frac);
            self.time_elapsed_label.set_text(&format_time(elapsed));
            self.time_total_label.set_text(&format_time(duration));
        }
    }

    pub fn set_playing(&self, playing: bool) {
        *self.is_playing.borrow_mut() = playing;
        if playing {
            self.play_pause_btn
                .set_icon_name("media-playback-pause-symbolic");
        } else {
            self.play_pause_btn
                .set_icon_name("media-playback-start-symbolic");
        }
    }

    pub fn set_thumbnail(&self, texture: &Texture) {
        self.thumb_pic.set_paintable(Some(texture));
    }

    pub fn connect_close(&self, callback: impl Fn() + 'static) {
        self.close_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_play_pause(&self, callback: impl Fn() + 'static) {
        self.play_pause_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_next(&self, callback: impl Fn() + 'static) {
        self.next_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_prev(&self, callback: impl Fn() + 'static) {
        self.prev_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_shuffle(&self, callback: impl Fn() + 'static) {
        self.shuffle_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_clear(&self, callback: impl Fn() + 'static) {
        self.clear_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_jump_to(&self, mut callback: impl FnMut(usize) + 'static) {
        // Will be called when row clicked
        let _ = &mut callback;
    }

    pub fn update_ui(&self) {
        let queue = self.queue.borrow();
        let cur_idx = *self.current_index.borrow();
        let count = queue.len();

        if count == 0 {
            self.root.set_visible(false);
            return;
        }

        self.root.set_visible(true);

        if let Some(cur) = queue.get(cur_idx) {
            self.title_label.set_text(&cur.title);
            self.channel_btn.set_label(&cur.channel_name);
            self.queue_pos_label
                .set_text(&format!("Queue · {} of {}", cur_idx + 1, count));
        }

        let is_exp = *self.is_expanded.borrow();
        if is_exp {
            self.expand_btn.set_label(&format!("Queue · {count} ⌵"));
        } else {
            self.expand_btn.set_label(&format!("Queue · {count} ⌃"));
        }

        self.queue_summary_label
            .set_text(&format!("{count} videos · drag to reorder"));

        // Render queue rows
        while let Some(child) = self.queue_list.first_child() {
            self.queue_list.remove(&child);
        }

        for (i, item) in queue.iter().enumerate() {
            let row = GtkBox::new(Orientation::Horizontal, 8);
            row.add_css_class("yr-queue-row");
            if i == cur_idx {
                row.add_css_class("current");
            }

            let handle = gtk4::Image::from_icon_name("open-menu-symbolic");
            handle.add_css_class("yr-meta");

            let thumb = Picture::new();
            thumb.set_width_request(48);
            thumb.set_height_request(27);
            thumb.set_content_fit(gtk4::ContentFit::Cover);
            thumb.add_css_class("yr-thumb");

            let text_box = GtkBox::new(Orientation::Vertical, 1);
            text_box.set_hexpand(true);

            let row_title = Label::new(Some(&item.title));
            row_title.set_halign(Align::Start);
            row_title.set_xalign(0.0);
            row_title.set_lines(1);
            row_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);

            let row_meta = if i == cur_idx {
                format!("{} · Playing", item.channel_name)
            } else {
                item.channel_name.clone()
            };
            let row_chan = Label::new(Some(&row_meta));
            row_chan.add_css_class("yr-meta");
            row_chan.set_halign(Align::Start);
            row_chan.set_xalign(0.0);

            text_box.append(&row_title);
            text_box.append(&row_chan);

            let remove_btn = Button::builder()
                .icon_name("window-close-symbolic")
                .valign(Align::Center)
                .build();
            remove_btn.add_css_class("flat");

            row.append(&handle);
            row.append(&thumb);
            row.append(&text_box);
            row.append(&remove_btn);

            self.queue_list.append(&row);
        }
    }
}

fn format_time(seconds: f64) -> String {
    let s = seconds as u64;
    let m = s / 60;
    let rem_s = s % 60;
    format!("{}:{:02}", m, rem_s)
}
