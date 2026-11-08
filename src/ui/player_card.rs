use crate::backend::models::{format_duration, format_queue_total_duration, VideoSummary};
use crate::ui::image_loader::ImageLoader;
use adw::prelude::*;
use gtk4::{
    gdk, glib, Align, Box as GtkBox, Button, DragSource, DropTarget, EventControllerKey,
    GestureClick, Label, Orientation, Overlay, Picture, ProgressBar, ScrolledWindow, Separator,
};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SHUFFLE_SEED_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct PlayerCard {
    pub root: GtkBox,
    inner_clamp: adw::Clamp,
    image_loader: Rc<ImageLoader>,
    thumb_pic: Picture,
    thumb_overlay: Overlay,
    thumb_h_clamp: adw::Clamp,
    thumb_v_clamp: adw::Clamp,
    title_label: Label,
    channel_btn: Button,
    queue_pos_label: Label,
    progress_bar: ProgressBar,
    times_row: GtkBox,
    time_elapsed_label: Label,
    time_total_label: Label,
    controls_row: GtkBox,
    pub prev_btn: Button,
    pub play_pause_btn: Button,
    narrow_play_pause_btn: Button,
    pub next_btn: Button,
    pub close_btn: Button,
    pub expand_btn: Button,
    queue_box: GtkBox,
    queue_list: GtkBox,
    queue_summary_label: Label,
    shuffle_btn: Button,
    clear_btn: Button,
    is_expanded: Cell<bool>,
    is_playing: Cell<bool>,
    is_narrow: Cell<bool>,
    is_shorts_mode: Cell<bool>,
    queue: RefCell<Vec<VideoSummary>>,
    current_index: Cell<usize>,
    on_open_channel: RefCell<Option<Rc<dyn Fn(String, String)>>>,
    on_jump_to: RefCell<Option<Rc<dyn Fn(usize)>>>,
    on_remove_at: RefCell<Option<Rc<dyn Fn(usize)>>>,
    on_move_item: RefCell<Option<Rc<dyn Fn(usize, usize)>>>,
    on_empty_stop: RefCell<Option<Rc<dyn Fn()>>>,
}

impl PlayerCard {
    pub fn new(image_loader: Rc<ImageLoader>) -> Rc<Self> {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_width_request(370);
        root.set_halign(Align::End);
        root.set_valign(Align::End);
        root.set_focusable(true);
        root.add_css_class("yr-player");
        root.set_visible(false);

        let body = GtkBox::new(Orientation::Vertical, 10);
        body.set_hexpand(true);
        let inner_clamp = adw::Clamp::builder()
            .maximum_size(342)
            .tightening_threshold(342)
            .child(&body)
            .build();
        root.append(&inner_clamp);

        // --- Collapsed Top Row ---
        let top_row = GtkBox::new(Orientation::Horizontal, 12);
        top_row.set_valign(Align::Center);

        let thumb_pic = Picture::new();
        thumb_pic.set_can_shrink(true);
        thumb_pic.set_hexpand(true);
        thumb_pic.set_vexpand(true);
        thumb_pic.set_content_fit(gtk4::ContentFit::Cover);

        let thumb_overlay = Overlay::new();
        thumb_overlay.set_child(Some(&thumb_pic));
        thumb_overlay.set_size_request(96, 54);
        thumb_overlay.add_css_class("yr-thumb");

        let thumb_v_clamp = adw::Clamp::builder()
            .orientation(Orientation::Vertical)
            .maximum_size(54)
            .tightening_threshold(54)
            .child(&thumb_overlay)
            .build();
        let thumb_h_clamp = adw::Clamp::builder()
            .orientation(Orientation::Horizontal)
            .maximum_size(96)
            .tightening_threshold(96)
            .halign(Align::Start)
            .valign(Align::Center)
            .child(&thumb_v_clamp)
            .build();

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
        channel_btn.set_tooltip_text(Some("Open channel"));

        let queue_pos_label = Label::new(Some("Queue · 1 of 1"));
        queue_pos_label.add_css_class("yr-meta");
        queue_pos_label.set_halign(Align::Start);
        queue_pos_label.set_xalign(0.0);

        text_col.append(&title_label);
        text_col.append(&channel_btn);
        text_col.append(&queue_pos_label);

        // Slim narrow-mode play/pause button (hidden in wide mode)
        let narrow_play_pause_btn = Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .tooltip_text("Play / Pause")
            .valign(Align::Center)
            .visible(false)
            .build();
        narrow_play_pause_btn.add_css_class("yr-rail-btn");
        narrow_play_pause_btn.add_css_class("active");

        let close_btn = Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Stop playback and close player")
            .valign(Align::Start)
            .build();
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");

        top_row.append(&thumb_h_clamp);
        top_row.append(&text_col);
        top_row.append(&narrow_play_pause_btn);
        top_row.append(&close_btn);
        body.append(&top_row);

        // --- Progress Bar & Times ---
        let progress_bar = ProgressBar::new();
        progress_bar.add_css_class("yr-progress");
        progress_bar.set_fraction(0.0);
        body.append(&progress_bar);

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
        body.append(&times_row);

        // --- Controls Row ---
        let controls_row = GtkBox::new(Orientation::Horizontal, 8);
        controls_row.set_valign(Align::Center);

        let prev_btn = Button::builder()
            .icon_name("media-skip-backward-symbolic")
            .tooltip_text("Previous video")
            .build();
        prev_btn.add_css_class("flat");

        let play_pause_btn = Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .tooltip_text("Play / Pause (Space)")
            .build();
        play_pause_btn.add_css_class("yr-rail-btn");
        play_pause_btn.add_css_class("active");

        let next_btn = Button::builder()
            .icon_name("media-skip-forward-symbolic")
            .tooltip_text("Next video")
            .build();
        next_btn.add_css_class("flat");

        let ctrl_spacer = GtkBox::new(Orientation::Horizontal, 0);
        ctrl_spacer.set_hexpand(true);

        let expand_btn = Button::builder()
            .label("Queue · 0 ⌃")
            .tooltip_text("Show or hide queue")
            .build();
        expand_btn.add_css_class("yr-chip");

        controls_row.append(&prev_btn);
        controls_row.append(&play_pause_btn);
        controls_row.append(&next_btn);
        controls_row.append(&ctrl_spacer);
        controls_row.append(&expand_btn);
        body.append(&controls_row);

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
        queue_summary_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        queue_summary_label.set_hexpand(true);
        queue_summary_label.set_xalign(0.0);

        let shuffle_btn = Button::with_label("Shuffle");
        shuffle_btn.add_css_class("yr-chip");
        shuffle_btn.set_tooltip_text(Some("Shuffle upcoming videos"));

        let clear_btn = Button::with_label("Clear");
        clear_btn.add_css_class("yr-chip");
        clear_btn.set_tooltip_text(Some("Clear queue"));

        queue_header.append(&queue_summary_label);
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
        body.append(&queue_box);

        let card = Rc::new(Self {
            root,
            inner_clamp,
            image_loader,
            thumb_pic,
            thumb_overlay,
            thumb_h_clamp,
            thumb_v_clamp,
            title_label,
            channel_btn,
            queue_pos_label,
            progress_bar,
            times_row,
            time_elapsed_label,
            time_total_label,
            controls_row,
            prev_btn,
            play_pause_btn,
            narrow_play_pause_btn,
            next_btn,
            close_btn,
            expand_btn,
            queue_box,
            queue_list,
            queue_summary_label,
            shuffle_btn,
            clear_btn,
            is_expanded: Cell::new(false),
            is_playing: Cell::new(true),
            is_narrow: Cell::new(false),
            is_shorts_mode: Cell::new(false),
            queue: RefCell::new(Vec::new()),
            current_index: Cell::new(0),
            on_open_channel: RefCell::new(None),
            on_jump_to: RefCell::new(None),
            on_remove_at: RefCell::new(None),
            on_move_item: RefCell::new(None),
            on_empty_stop: RefCell::new(None),
        });

        // Expand/collapse toggle
        let card_weak = Rc::downgrade(&card);
        card.expand_btn.connect_clicked(move |_| {
            if let Some(c) = card_weak.upgrade() {
                let next_exp = !c.is_expanded.get();
                c.is_expanded.set(next_exp);
                if !c.is_narrow.get() {
                    c.queue_box.set_visible(next_exp);
                }
                let q_count = c.queue.borrow().len();
                c.expand_btn.set_label(&format!(
                    "Queue · {q_count} {}",
                    if next_exp { "⌵" } else { "⌃" }
                ));
            }
        });

        // Channel link click -> opens channel page
        let card_weak = Rc::downgrade(&card);
        card.channel_btn.connect_clicked(move |_| {
            if let Some(c) = card_weak.upgrade() {
                let maybe_chan = {
                    let q = c.queue.borrow();
                    q.get(c.current_index.get())
                        .map(|v| (v.channel_id.clone(), v.channel_name.clone()))
                };
                if let Some((cid, cname)) = maybe_chan {
                    if !cid.is_empty() {
                        if let Some(cb) = c.on_open_channel.borrow().as_ref().cloned() {
                            cb(cid, cname);
                        }
                    }
                }
            }
        });

        // Shuffle button
        let card_weak = Rc::downgrade(&card);
        card.shuffle_btn.connect_clicked(move |_| {
            if let Some(c) = card_weak.upgrade() {
                c.shuffle_queue();
            }
        });

        // Spacebar shortcut when PlayerCard has focus
        let key_ctrl = EventControllerKey::new();
        let play_btn_key = card.play_pause_btn.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::space {
                play_btn_key.emit_clicked();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        card.root.add_controller(key_ctrl);

        card
    }

    pub fn set_queue(self: &Rc<Self>, items: Vec<VideoSummary>, current_pos: usize, shorts_mode: bool) {
        self.is_shorts_mode.set(shorts_mode);
        let clamped_pos = if items.is_empty() {
            0
        } else {
            current_pos.min(items.len() - 1)
        };
        *self.queue.borrow_mut() = items;
        self.current_index.set(clamped_pos);
        self.reset_progress_labels();
        self.update_ui();
    }

    pub fn add_item(self: &Rc<Self>, item: VideoSummary) {
        self.queue.borrow_mut().push(item);
        self.update_ui();
    }

    pub fn current_video(&self) -> Option<VideoSummary> {
        self.queue.borrow().get(self.current_index.get()).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.borrow().is_empty()
    }

    pub fn set_current_index(self: &Rc<Self>, index: usize) {
        let len = self.queue.borrow().len();
        if index < len && index != self.current_index.get() {
            self.current_index.set(index);
            self.reset_progress_labels();
            self.update_ui();
        } else if index < len {
            self.update_ui();
        }
    }

    fn reset_progress_labels(&self) {
        self.progress_bar.set_fraction(0.0);
        self.time_elapsed_label.set_text("0:00");
        let dur_txt = self
            .queue
            .borrow()
            .get(self.current_index.get())
            .and_then(|v| v.duration_seconds)
            .map(format_duration)
            .unwrap_or_else(|| "0:00".to_string());
        self.time_total_label.set_text(&dur_txt);
    }

    pub fn set_progress(&self, elapsed: f64, duration: f64) {
        let safe_elapsed = elapsed.max(0.0);
        self.time_elapsed_label
            .set_text(&format_duration(safe_elapsed as u64));
        if duration > 0.0 {
            let frac = (safe_elapsed / duration).clamp(0.0, 1.0);
            self.progress_bar.set_fraction(frac);
            self.time_total_label
                .set_text(&format_duration(duration as u64));
        }
    }

    pub fn set_playing(&self, playing: bool) {
        self.is_playing.set(playing);
        let icon = if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        };
        self.play_pause_btn.set_icon_name(icon);
        self.narrow_play_pause_btn.set_icon_name(icon);
        if playing && !self.queue.borrow().is_empty() {
            self.root.set_visible(true);
        }
    }

    pub fn set_narrow_mode(self: &Rc<Self>, narrow: bool) {
        self.is_narrow.set(narrow);
        if narrow {
            self.root.set_width_request(-1);
            self.root.set_halign(Align::Fill);
            self.root.add_css_class("yr-player-narrow");
            self.inner_clamp.set_maximum_size(2400);
            self.inner_clamp.set_tightening_threshold(2400);
            self.thumb_overlay.set_size_request(64, 36);
            self.thumb_h_clamp.set_maximum_size(64);
            self.thumb_h_clamp.set_tightening_threshold(64);
            self.thumb_v_clamp.set_maximum_size(36);
            self.thumb_v_clamp.set_tightening_threshold(36);
            self.close_btn.set_valign(Align::Center);
            self.narrow_play_pause_btn.set_visible(true);
            self.progress_bar.set_visible(false);
            self.times_row.set_visible(false);
            self.controls_row.set_visible(false);
            self.queue_box.set_visible(false);
        } else {
            self.root.set_width_request(370);
            self.root.set_halign(Align::End);
            self.root.remove_css_class("yr-player-narrow");
            self.inner_clamp.set_maximum_size(342);
            self.inner_clamp.set_tightening_threshold(342);
            self.thumb_overlay.set_size_request(96, 54);
            self.thumb_h_clamp.set_maximum_size(96);
            self.thumb_h_clamp.set_tightening_threshold(96);
            self.thumb_v_clamp.set_maximum_size(54);
            self.thumb_v_clamp.set_tightening_threshold(54);
            self.close_btn.set_valign(Align::Start);
            self.narrow_play_pause_btn.set_visible(false);
            self.progress_bar.set_visible(true);
            self.times_row.set_visible(true);
            self.controls_row.set_visible(true);
            self.queue_box.set_visible(self.is_expanded.get());
        }
        self.update_header_labels();
    }

    pub fn connect_close(&self, callback: impl Fn() + 'static) {
        self.close_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_play_pause(&self, callback: impl Fn() + 'static) {
        let cb = Rc::new(callback);
        let cb_wide = cb.clone();
        self.play_pause_btn.connect_clicked(move |_| cb_wide());
        self.narrow_play_pause_btn.connect_clicked(move |_| cb());
    }

    pub fn connect_next(&self, callback: impl Fn() + 'static) {
        self.next_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_prev(&self, callback: impl Fn() + 'static) {
        self.prev_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_clear(&self, callback: impl Fn() + 'static) {
        self.clear_btn.connect_clicked(move |_| callback());
    }

    pub fn connect_open_channel(&self, callback: impl Fn(String, String) + 'static) {
        *self.on_open_channel.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn connect_jump_to(&self, callback: impl Fn(usize) + 'static) {
        *self.on_jump_to.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn connect_remove_at(&self, callback: impl Fn(usize) + 'static) {
        *self.on_remove_at.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn connect_move_item(&self, callback: impl Fn(usize, usize) + 'static) {
        *self.on_move_item.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn connect_empty_stop(&self, callback: impl Fn() + 'static) {
        *self.on_empty_stop.borrow_mut() = Some(Rc::new(callback));
    }

    fn move_item_internal(&self, from_idx: usize, to_idx: usize) -> bool {
        let mut q = self.queue.borrow_mut();
        let len = q.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }
        let item = q.remove(from_idx);
        q.insert(to_idx, item);

        let cur = self.current_index.get();
        let new_cur = if cur == from_idx {
            to_idx
        } else if from_idx < cur && to_idx >= cur {
            cur - 1
        } else if from_idx > cur && to_idx <= cur {
            cur + 1
        } else {
            cur
        };
        self.current_index.set(new_cur);
        drop(q);

        if let Some(cb) = self.on_move_item.borrow().as_ref().cloned() {
            cb(from_idx, to_idx);
        }
        true
    }

    pub fn move_item(self: &Rc<Self>, from_idx: usize, to_idx: usize) {
        if self.move_item_internal(from_idx, to_idx) {
            self.update_ui();
        }
    }

    pub fn remove_item_at(self: &Rc<Self>, index: usize) {
        let len_after = {
            let mut q = self.queue.borrow_mut();
            if index >= q.len() {
                return;
            }
            q.remove(index);
            q.len()
        };

        if let Some(cb) = self.on_remove_at.borrow().as_ref().cloned() {
            cb(index);
        }

        if len_after == 0 {
            self.current_index.set(0);
            self.root.set_visible(false);
            if let Some(cb) = self.on_empty_stop.borrow().as_ref().cloned() {
                cb();
            }
            return;
        }

        let cur = self.current_index.get();
        if index < cur {
            self.current_index.set(cur - 1);
        } else if cur >= len_after {
            self.current_index.set(len_after - 1);
        }

        self.update_ui();
    }

    pub fn shuffle_queue(self: &Rc<Self>) {
        let len = self.queue.borrow().len();
        if len <= 1 {
            return;
        }

        // Keep currently playing video at index 0 so playback continues smoothly
        let cur = self.current_index.get();
        if cur != 0 {
            self.move_item_internal(cur, 0);
        }

        let mut state = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            ^ SHUFFLE_SEED_COUNTER
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_mul(0xBF58_476D_1CE4_E5B9);
        if state == 0 {
            state = 0x9E37_79B9_7F4A_7C15;
        }

        for i in 1..len.saturating_sub(1) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let span = len - i;
            let j = i + (state as usize % span);
            if j != i {
                self.move_item_internal(j, i);
            }
        }

        self.update_ui();
    }

    fn update_header_labels(&self) {
        let queue = self.queue.borrow();
        let count = queue.len();
        if count == 0 {
            return;
        }
        let cur_idx = self.current_index.get().min(count - 1);
        if let Some(cur) = queue.get(cur_idx) {
            self.title_label.set_text(&cur.title);
            self.channel_btn.set_label(&cur.channel_name);
            self.channel_btn.set_sensitive(!cur.channel_id.is_empty());

            let prefix = if self.is_shorts_mode.get() {
                "Short"
            } else {
                "Queue"
            };
            if self.is_narrow.get() {
                self.queue_pos_label
                    .set_text(&format!("{} of {}", cur_idx + 1, count));
            } else {
                self.queue_pos_label
                    .set_text(&format!("{prefix} · {} of {}", cur_idx + 1, count));
            }
        }
    }

    pub fn update_ui(self: &Rc<Self>) {
        let (count, cur_idx, cur_thumb, total_secs) = {
            let queue = self.queue.borrow();
            let count = queue.len();
            if count == 0 {
                self.root.set_visible(false);
                return;
            }
            let cur_idx = self.current_index.get().min(count - 1);
            let cur_thumb = queue
                .get(cur_idx)
                .map(|v| (v.id.clone(), v.thumbnail_url.clone()));
            let total_secs: u64 = queue.iter().filter_map(|v| v.duration_seconds).sum();
            (count, cur_idx, cur_thumb, total_secs)
        };

        self.root.set_visible(true);
        self.update_header_labels();

        if let Some((vid, url)) = cur_thumb {
            if url.is_empty() {
                self.thumb_pic.set_paintable(gtk4::gdk::Texture::NONE);
            } else {
                self.image_loader
                    .load_into_picture(&vid, &url, &self.thumb_pic);
            }
        }

        let is_exp = self.is_expanded.get();
        self.expand_btn.set_label(&format!(
            "Queue · {count} {}",
            if is_exp { "⌵" } else { "⌃" }
        ));

        let word = if count == 1 { "video" } else { "videos" };
        let dur_part = format_queue_total_duration(total_secs);
        if dur_part.is_empty() {
            self.queue_summary_label
                .set_text(&format!("{count} {word} · drag to reorder"));
        } else {
            self.queue_summary_label
                .set_text(&format!("{count} {word} · {dur_part} · drag to reorder"));
        }

        // Rebuild queue rows
        while let Some(child) = self.queue_list.first_child() {
            self.queue_list.remove(&child);
        }

        let items_snapshot = self.queue.borrow().clone();
        for (i, item) in items_snapshot.iter().enumerate() {
            let row = GtkBox::new(Orientation::Horizontal, 8);
            row.add_css_class("yr-queue-row");
            if i == cur_idx {
                row.add_css_class("current");
            }

            let click_area = GtkBox::new(Orientation::Horizontal, 8);
            click_area.set_hexpand(true);

            let handle = gtk4::Image::from_icon_name("list-drag-handle-symbolic");
            handle.add_css_class("yr-meta");
            handle.set_tooltip_text(Some("Drag to reorder"));

            let thumb = Picture::new();
            thumb.set_can_shrink(true);
            thumb.set_hexpand(true);
            thumb.set_vexpand(true);
            thumb.set_content_fit(gtk4::ContentFit::Cover);

            let row_thumb_overlay = Overlay::new();
            row_thumb_overlay.set_child(Some(&thumb));
            row_thumb_overlay.add_css_class("yr-thumb");
            row_thumb_overlay.set_size_request(64, 36);

            let row_v_clamp = adw::Clamp::builder()
                .orientation(Orientation::Vertical)
                .maximum_size(36)
                .tightening_threshold(36)
                .child(&row_thumb_overlay)
                .build();
            let row_h_clamp = adw::Clamp::builder()
                .orientation(Orientation::Horizontal)
                .maximum_size(64)
                .tightening_threshold(64)
                .valign(Align::Center)
                .child(&row_v_clamp)
                .build();

            if !item.thumbnail_url.is_empty() {
                self.image_loader
                    .load_into_picture(&item.id, &item.thumbnail_url, &thumb);
            }

            let text_box = GtkBox::new(Orientation::Vertical, 1);
            text_box.set_hexpand(true);
            text_box.set_valign(Align::Center);

            let row_title = Label::new(Some(&item.title));
            row_title.add_css_class("yr-title");
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
            row_chan.set_ellipsize(gtk4::pango::EllipsizeMode::End);

            text_box.append(&row_title);
            text_box.append(&row_chan);

            click_area.append(&handle);
            click_area.append(&row_h_clamp);
            click_area.append(&text_box);

            // Click row -> jump to track `i`
            let row_click = GestureClick::new();
            row_click.set_button(1);
            let card_weak = Rc::downgrade(self);
            row_click.connect_released(move |_, _, _, _| {
                if let Some(card) = card_weak.upgrade() {
                    card.set_current_index(i);
                    if let Some(cb) = card.on_jump_to.borrow().as_ref().cloned() {
                        cb(i);
                    }
                }
            });
            click_area.add_controller(row_click);

            // Remove (x) button -> remove track `i`
            let remove_btn = Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("Remove from queue")
                .valign(Align::Center)
                .build();
            remove_btn.add_css_class("flat");
            let card_weak = Rc::downgrade(self);
            remove_btn.connect_clicked(move |_| {
                if let Some(card) = card_weak.upgrade() {
                    card.remove_item_at(i);
                }
            });

            row.append(&click_area);
            row.append(&remove_btn);

            // Drag-and-drop reordering
            let drag_source = DragSource::new();
            drag_source.set_actions(gdk::DragAction::MOVE);
            let drag_idx = i as u32;
            drag_source.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(&drag_idx.to_value()))
            });
            row.add_controller(drag_source);

            let drop_target = DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
            let row_enter = row.clone();
            drop_target.connect_enter(move |_, _, _| {
                row_enter.add_css_class("drop-target");
                gdk::DragAction::MOVE
            });
            let row_leave = row.clone();
            drop_target.connect_leave(move |_| {
                row_leave.remove_css_class("drop-target");
            });
            let row_drop = row.clone();
            let card_weak = Rc::downgrade(self);
            let target_idx = i;
            drop_target.connect_drop(move |_, val, _, _| {
                row_drop.remove_css_class("drop-target");
                if let Ok(from_u32) = val.get::<u32>() {
                    if let Some(card) = card_weak.upgrade() {
                        card.move_item(from_u32 as usize, target_idx);
                        return true;
                    }
                }
                false
            });
            row.add_controller(drop_target);

            self.queue_list.append(&row);
        }
    }
}
