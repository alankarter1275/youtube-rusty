use crate::backend::models::VideoSummary;
use crate::player::mpv::Quality;
use crate::ui::image_loader::ImageLoader;
use gtk4::gdk::Rectangle;
use gtk4::{
    Align, Box as GtkBox, Button, GestureClick, GestureLongPress, Label, Orientation, Overlay,
    Picture, Popover, Separator, ToggleButton,
};
use libadwaita as adw;
use adw::prelude::*;
use std::rc::Rc;

#[derive(Clone)]
pub struct CardActions {
    pub image_loader: Rc<ImageLoader>,
    pub default_quality: Rc<dyn Fn() -> Quality>,
    pub is_watch_later: Rc<dyn Fn(&str) -> bool>,
    pub on_play: Rc<dyn Fn(VideoSummary, Quality)>,
    pub on_add_queue: Rc<dyn Fn(VideoSummary)>,
    pub on_toggle_watch_later: Rc<dyn Fn(VideoSummary) -> bool>,
    pub on_download: Rc<dyn Fn(VideoSummary, Quality)>,
    pub on_open_channel: Rc<dyn Fn(String, String)>,
    pub on_toast: Rc<dyn Fn(String)>,
}

pub struct DetailsSheet;

impl DetailsSheet {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        summary: &VideoSummary,
        actions: &CardActions,
    ) {
        let dialog = adw::Dialog::builder()
            .content_width(520)
            .title(&summary.title)
            .build();
        dialog.add_css_class("yr-sheet");

        let content_box = GtkBox::new(Orientation::Vertical, 14);
        content_box.set_margin_top(16);
        content_box.set_margin_bottom(20);
        content_box.set_margin_start(20);
        content_box.set_margin_end(20);

        // Top Thumbnail (16:9)
        let picture = Picture::new();
        picture.set_width_request(480);
        picture.set_height_request(270);
        picture.set_content_fit(gtk4::ContentFit::Cover);
        actions
            .image_loader
            .load_into_picture(&summary.id, &summary.thumbnail_url, &picture);

        let thumb_overlay = Overlay::new();
        thumb_overlay.set_child(Some(&picture));
        thumb_overlay.add_css_class("yr-thumb");

        if !summary.duration_text.is_empty() {
            let dur_label = Label::new(Some(&summary.duration_text));
            dur_label.add_css_class("yr-duration");
            dur_label.set_halign(Align::End);
            dur_label.set_valign(Align::End);
            thumb_overlay.add_overlay(&dur_label);
        }

        content_box.append(&thumb_overlay);

        // Title (Playfair 22, max 2 lines)
        let title_label = Label::new(Some(&summary.title));
        title_label.add_css_class("yr-sheet-title");
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        content_box.append(&title_label);

        // Channel row (Avatar + Name + Views/Age + Open Channel button)
        let channel_row = GtkBox::new(Orientation::Horizontal, 12);
        channel_row.set_valign(Align::Center);

        let avatar = adw::Avatar::new(36, Some(&summary.channel_name), true);
        actions.image_loader.load_into_avatar(
            &summary.channel_id,
            &summary.channel_avatar_url,
            &avatar,
        );
        channel_row.append(&avatar);

        let channel_info = GtkBox::new(Orientation::Vertical, 2);
        let channel_name = Label::new(Some(&summary.channel_name));
        channel_name.set_halign(Align::Start);
        channel_name.set_xalign(0.0);
        channel_name.add_css_class("yr-title");

        let stats_text = match (
            summary.view_count_text.is_empty(),
            summary.published_text.is_empty(),
        ) {
            (false, false) => format!("{} · {}", summary.view_count_text, summary.published_text),
            (false, true) => summary.view_count_text.clone(),
            (true, false) => summary.published_text.clone(),
            (true, true) => String::new(),
        };
        let stats_label = Label::new(Some(&stats_text));
        stats_label.add_css_class("yr-meta");
        stats_label.set_halign(Align::Start);
        stats_label.set_xalign(0.0);

        channel_info.append(&channel_name);
        if !stats_text.is_empty() {
            channel_info.append(&stats_label);
        }
        channel_row.append(&channel_info);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        channel_row.append(&spacer);

        let open_channel_btn = Button::with_label("Open channel");
        open_channel_btn.add_css_class("yr-btn");
        open_channel_btn.set_sensitive(!summary.channel_id.is_empty());
        let cid = summary.channel_id.clone();
        let cname = summary.channel_name.clone();
        let dlg_weak = dialog.downgrade();
        let on_open_channel = actions.on_open_channel.clone();
        open_channel_btn.connect_clicked(move |_| {
            if let Some(dlg) = dlg_weak.upgrade() {
                dlg.close();
            }
            on_open_channel(cid.clone(), cname.clone());
        });
        channel_row.append(&open_channel_btn);

        content_box.append(&channel_row);

        // One-line description snippet
        if !summary.description_snippet.is_empty() {
            let desc_label = Label::new(Some(&summary.description_snippet));
            desc_label.add_css_class("yr-meta");
            desc_label.set_halign(Align::Start);
            desc_label.set_xalign(0.0);
            desc_label.set_wrap(true);
            desc_label.set_lines(1);
            desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            content_box.append(&desc_label);
        }

        // Play row: Primary "Play in MPV" button + segmented quality control
        let play_row = GtkBox::new(Orientation::Horizontal, 12);
        play_row.set_margin_top(4);

        let play_btn = Button::builder().label("Play in MPV").hexpand(true).build();
        play_btn.add_css_class("yr-btn");
        play_btn.add_css_class("primary");

        let quality_box = GtkBox::new(Orientation::Horizontal, 2);
        quality_box.add_css_class("yr-segmented");

        let q_360 = ToggleButton::with_label("360p");
        q_360.add_css_class("yr-chip");

        let q_720 = ToggleButton::with_label("720p");
        q_720.add_css_class("yr-chip");
        q_720.set_group(Some(&q_360));

        let q_1080 = ToggleButton::with_label("1080p");
        q_1080.add_css_class("yr-chip");
        q_1080.set_group(Some(&q_360));

        let q_audio = ToggleButton::with_label("Audio");
        q_audio.add_css_class("yr-chip");
        q_audio.set_group(Some(&q_360));

        match (actions.default_quality)() {
            Quality::P360 => q_360.set_active(true),
            Quality::P720 => q_720.set_active(true),
            Quality::P1080 => q_1080.set_active(true),
            Quality::AudioOnly => q_audio.set_active(true),
        }

        quality_box.append(&q_360);
        quality_box.append(&q_720);
        quality_box.append(&q_1080);
        quality_box.append(&q_audio);

        play_row.append(&play_btn);
        play_row.append(&quality_box);
        content_box.append(&play_row);

        // Secondary actions: 4 equal buttons
        let actions_box = GtkBox::new(Orientation::Horizontal, 8);
        actions_box.set_homogeneous(true);
        actions_box.set_margin_top(2);

        let queue_btn = Button::with_label("Add to queue");
        queue_btn.add_css_class("yr-btn");

        let initially_saved = (actions.is_watch_later)(&summary.id);
        let watch_later_btn = Button::with_label(if initially_saved {
            "Saved ✓"
        } else {
            "Watch later"
        });
        watch_later_btn.add_css_class("yr-btn");

        let copy_btn = Button::with_label("Copy link");
        copy_btn.add_css_class("yr-btn");

        let download_btn = Button::with_label("Download");
        download_btn.add_css_class("yr-btn");

        actions_box.append(&queue_btn);
        actions_box.append(&watch_later_btn);
        actions_box.append(&copy_btn);
        actions_box.append(&download_btn);
        content_box.append(&actions_box);

        dialog.set_child(Some(&content_box));

        let selected_quality = {
            let q_720_c = q_720.clone();
            let q_1080_c = q_1080.clone();
            let q_audio_c = q_audio.clone();
            Rc::new(move || {
                if q_audio_c.is_active() {
                    Quality::AudioOnly
                } else if q_1080_c.is_active() {
                    Quality::P1080
                } else if q_720_c.is_active() {
                    Quality::P720
                } else {
                    Quality::P360
                }
            })
        };

        let sum_play = summary.clone();
        let dlg_weak2 = dialog.downgrade();
        let on_play = actions.on_play.clone();
        let sel_q_play = selected_quality.clone();
        play_btn.connect_clicked(move |_| {
            let q = sel_q_play();
            if let Some(dlg) = dlg_weak2.upgrade() {
                dlg.close();
            }
            on_play(sum_play.clone(), q);
        });

        let sum_q = summary.clone();
        let on_add_q = actions.on_add_queue.clone();
        queue_btn.connect_clicked(move |_| {
            on_add_q(sum_q.clone());
        });

        let sum_wl = summary.clone();
        let on_wl = actions.on_toggle_watch_later.clone();
        watch_later_btn.connect_clicked(move |btn| {
            let now_saved = on_wl(sum_wl.clone());
            btn.set_label(if now_saved { "Saved ✓" } else { "Watch later" });
        });

        let vid_id = summary.id.clone();
        let on_toast_copy = actions.on_toast.clone();
        copy_btn.connect_clicked(move |btn| {
            let url = format!("https://www.youtube.com/watch?v={vid_id}");
            btn.display().clipboard().set_text(&url);
            on_toast_copy("Copied link to clipboard".to_string());
        });

        let sum_dl = summary.clone();
        let on_dl = actions.on_download.clone();
        let sel_q_dl = selected_quality.clone();
        download_btn.connect_clicked(move |_| {
            on_dl(sum_dl.clone(), sel_q_dl());
        });

        dialog.present(Some(parent));
    }
}

pub struct ContextMenu;

impl ContextMenu {
    pub fn attach(
        target: &impl IsA<gtk4::Widget>,
        summary: &VideoSummary,
        actions: &CardActions,
    ) {
        let popover = Popover::new();
        popover.set_parent(target);
        popover.set_has_arrow(false);

        let pop_cleanup = popover.clone();
        target.connect_destroy(move |_| {
            pop_cleanup.unparent();
        });

        let menu_box = GtkBox::new(Orientation::Vertical, 2);
        menu_box.set_width_request(210);

        let play_btn = create_menu_item("Play in MPV");
        let queue_btn = create_menu_item("Add to queue");
        let watch_later_btn = create_menu_item("Watch later");
        let sep = Separator::new(Orientation::Horizontal);
        sep.set_margin_top(4);
        sep.set_margin_bottom(4);
        let copy_btn = create_menu_item("Copy link");
        let channel_btn = create_menu_item("Open channel");
        channel_btn.set_sensitive(!summary.channel_id.is_empty());
        let dl_btn = create_menu_item("Download");

        menu_box.append(&play_btn);
        menu_box.append(&queue_btn);
        menu_box.append(&watch_later_btn);
        menu_box.append(&sep);
        menu_box.append(&copy_btn);
        menu_box.append(&channel_btn);
        menu_box.append(&dl_btn);

        popover.set_child(Some(&menu_box));

        let pop_weak = popover.downgrade();
        let sum_play = summary.clone();
        let on_play = actions.on_play.clone();
        let def_q = actions.default_quality.clone();
        play_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_play(sum_play.clone(), def_q());
        });

        let pop_weak = popover.downgrade();
        let sum_q = summary.clone();
        let on_add_q = actions.on_add_queue.clone();
        queue_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_add_q(sum_q.clone());
        });

        let pop_weak = popover.downgrade();
        let sum_wl = summary.clone();
        let on_wl = actions.on_toggle_watch_later.clone();
        watch_later_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            let _ = on_wl(sum_wl.clone());
        });

        let pop_weak = popover.downgrade();
        let vid_id = summary.id.clone();
        let on_toast = actions.on_toast.clone();
        copy_btn.connect_clicked(move |btn| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            let url = format!("https://www.youtube.com/watch?v={vid_id}");
            btn.display().clipboard().set_text(&url);
            on_toast("Copied link to clipboard".to_string());
        });

        let pop_weak = popover.downgrade();
        let cid = summary.channel_id.clone();
        let cname = summary.channel_name.clone();
        let on_chan = actions.on_open_channel.clone();
        channel_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_chan(cid.clone(), cname.clone());
        });

        let pop_weak = popover.downgrade();
        let sum_dl = summary.clone();
        let on_dl = actions.on_download.clone();
        let def_q_dl = actions.default_quality.clone();
        dl_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_dl(sum_dl.clone(), def_q_dl());
        });

        let right_click = GestureClick::new();
        right_click.set_button(3);
        let pop_weak = popover.downgrade();
        let wl_btn_rc = watch_later_btn.clone();
        let is_wl_rc = actions.is_watch_later.clone();
        let vid_rc = summary.id.clone();
        right_click.connect_pressed(move |_, _, x, y| {
            if let Some(pop) = pop_weak.upgrade() {
                update_menu_item_label(
                    &wl_btn_rc,
                    if is_wl_rc(&vid_rc) {
                        "Remove Watch later"
                    } else {
                        "Watch later"
                    },
                );
                let rect = Rectangle::new(x as i32, y as i32, 1, 1);
                pop.set_pointing_to(Some(&rect));
                pop.popup();
            }
        });
        target.add_controller(right_click);

        let long_press = GestureLongPress::new();
        let pop_weak2 = popover.downgrade();
        let wl_btn_lp = watch_later_btn;
        let is_wl_lp = actions.is_watch_later.clone();
        let vid_lp = summary.id.clone();
        long_press.connect_pressed(move |_, x, y| {
            if let Some(pop) = pop_weak2.upgrade() {
                update_menu_item_label(
                    &wl_btn_lp,
                    if is_wl_lp(&vid_lp) {
                        "Remove Watch later"
                    } else {
                        "Watch later"
                    },
                );
                let rect = Rectangle::new(x as i32, y as i32, 1, 1);
                pop.set_pointing_to(Some(&rect));
                pop.popup();
            }
        });
        target.add_controller(long_press);
    }
}

fn create_menu_item(label: &str) -> Button {
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

fn update_menu_item_label(btn: &Button, text: &str) {
    btn.set_label(text);
    if let Some(child) = btn.child() {
        if let Some(lbl) = child.downcast_ref::<Label>() {
            lbl.set_halign(Align::Start);
            lbl.set_xalign(0.0);
        }
    }
}
