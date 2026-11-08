use crate::backend::models::VideoSummary;
use crate::player::mpv::Quality;
use gtk4::gdk::{Rectangle, Texture};
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, GestureClick, GestureLongPress, Label, Orientation, Overlay,
    Picture, Popover, Separator, ToggleButton,
};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

pub struct DetailsSheet;

impl DetailsSheet {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        summary: &VideoSummary,
        thumb_texture: Option<&Texture>,
        avatar_texture: Option<&Texture>,
        on_play: impl Fn(String, String, Quality) + 'static,
        on_add_queue: impl Fn(VideoSummary) + 'static,
        on_toast: impl Fn(String) + 'static,
        on_open_channel: impl Fn(String, String) + 'static,
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
        if let Some(tex) = thumb_texture {
            picture.set_paintable(Some(tex));
        }

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
        title_label.add_css_class("yr-h1");
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
        if let Some(tex) = avatar_texture {
            avatar.set_custom_image(Some(tex));
        }
        channel_row.append(&avatar);

        let channel_info = GtkBox::new(Orientation::Vertical, 2);
        let channel_name = Label::new(Some(&summary.channel_name));
        channel_name.set_halign(Align::Start);
        channel_name.set_xalign(0.0);
        channel_name.add_css_class("heading");

        let stats_text = if !summary.published_text.is_empty() {
            format!("{} · {}", summary.view_count_text, summary.published_text)
        } else {
            summary.view_count_text.clone()
        };
        let stats_label = Label::new(Some(&stats_text));
        stats_label.add_css_class("yr-muted");
        stats_label.set_halign(Align::Start);
        stats_label.set_xalign(0.0);

        channel_info.append(&channel_name);
        channel_info.append(&stats_label);
        channel_row.append(&channel_info);

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        channel_row.append(&spacer);

        let open_channel_btn = Button::with_label("Open channel");
        open_channel_btn.add_css_class("yr-btn");
        let cid = summary.channel_id.clone();
        let cname = summary.channel_name.clone();
        let dlg_weak = dialog.downgrade();
        let on_open_channel_rc = Rc::new(on_open_channel);
        let on_open_channel_cl = on_open_channel_rc.clone();
        open_channel_btn.connect_clicked(move |_| {
            if let Some(dlg) = dlg_weak.upgrade() {
                dlg.close();
            }
            on_open_channel_cl(cid.clone(), cname.clone());
        });
        channel_row.append(&open_channel_btn);

        content_box.append(&channel_row);

        // One-line description snippet
        if !summary.description_snippet.is_empty() {
            let desc_label = Label::new(Some(&summary.description_snippet));
            desc_label.add_css_class("yr-muted");
            desc_label.set_halign(Align::Start);
            desc_label.set_xalign(0.0);
            desc_label.set_wrap(true);
            desc_label.set_lines(1);
            desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            content_box.append(&desc_label);
        }

        // Play row: Big primary "Play in MPV" button + segmented quality control
        let play_row = GtkBox::new(Orientation::Horizontal, 12);
        play_row.set_margin_top(4);

        let play_btn = Button::builder()
            .label("Play in MPV")
            .hexpand(true)
            .build();
        play_btn.add_css_class("yr-btn");
        play_btn.add_css_class("primary");

        // Segmented quality control
        let quality_box = GtkBox::new(Orientation::Horizontal, 2);
        quality_box.add_css_class("yr-segmented");

        let q_360 = ToggleButton::with_label("360p");
        q_360.add_css_class("yr-chip");
        q_360.set_active(true);

        let q_720 = ToggleButton::with_label("720p");
        q_720.add_css_class("yr-chip");
        q_720.set_group(Some(&q_360));

        let q_1080 = ToggleButton::with_label("1080p");
        q_1080.add_css_class("yr-chip");
        q_1080.set_group(Some(&q_360));

        let q_audio = ToggleButton::with_label("Audio");
        q_audio.add_css_class("yr-chip");
        q_audio.set_group(Some(&q_360));

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

        let watch_later_btn = Button::with_label("Watch later");
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

        // Connect button handlers
        let on_play_rc = Rc::new(on_play);
        let on_add_q_rc = Rc::new(on_add_queue);
        let on_toast_rc = Rc::new(on_toast);

        let vid_id = summary.id.clone();
        let vid_title = summary.title.clone();
        let dlg_weak2 = dialog.downgrade();
        let on_play_cl = on_play_rc.clone();
        let q_720_cl = q_720.clone();
        let q_1080_cl = q_1080.clone();
        let q_audio_cl = q_audio.clone();

        play_btn.connect_clicked(move |_| {
            let quality = if q_audio_cl.is_active() {
                Quality::AudioOnly
            } else if q_1080_cl.is_active() {
                Quality::P1080
            } else if q_720_cl.is_active() {
                Quality::P720
            } else {
                Quality::P360
            };

            if let Some(dlg) = dlg_weak2.upgrade() {
                dlg.close();
            }
            on_play_cl(vid_id.clone(), vid_title.clone(), quality);
        });

        let summary_q = summary.clone();
        let on_add_q_cl = on_add_q_rc.clone();
        queue_btn.connect_clicked(move |_| {
            on_add_q_cl(summary_q.clone());
        });

        let on_toast_wl = on_toast_rc.clone();
        watch_later_btn.connect_clicked(move |_| {
            on_toast_wl("Saved to Watch Later".to_string());
        });

        let vid_id2 = summary.id.clone();
        let on_toast_copy = on_toast_rc.clone();
        copy_btn.connect_clicked(move |btn| {
            let url = format!("https://www.youtube.com/watch?v={vid_id2}");
            let display = btn.display();
            display.clipboard().set_text(&url);
            on_toast_copy("Copied link to clipboard".to_string());
        });

        let on_toast_dl = on_toast_rc.clone();
        download_btn.connect_clicked(move |_| {
            on_toast_dl("Download started in background".to_string());
        });

        dialog.present(Some(parent));
    }
}

pub struct ContextMenu;

impl ContextMenu {
    pub fn attach(
        target: &impl IsA<gtk4::Widget>,
        summary: &VideoSummary,
        on_play: impl Fn(String, String, Quality) + 'static,
        on_add_queue: impl Fn(VideoSummary) + 'static,
        on_toast: impl Fn(String) + 'static,
        on_open_channel: impl Fn(String, String) + 'static,
    ) {
        let popover = Popover::new();
        popover.set_parent(target);
        popover.set_has_arrow(false);

        let menu_box = GtkBox::new(Orientation::Vertical, 2);
        menu_box.set_width_request(200);

        let play_btn = create_menu_item("Play in MPV");
        let queue_btn = create_menu_item("Add to queue");
        let watch_later_btn = create_menu_item("Watch later");
        let sep = Separator::new(Orientation::Horizontal);
        sep.set_margin_top(4);
        sep.set_margin_bottom(4);
        let copy_btn = create_menu_item("Copy link");
        let channel_btn = create_menu_item("Open channel");
        let dl_btn = create_menu_item("Download");

        menu_box.append(&play_btn);
        menu_box.append(&queue_btn);
        menu_box.append(&watch_later_btn);
        menu_box.append(&sep);
        menu_box.append(&copy_btn);
        menu_box.append(&channel_btn);
        menu_box.append(&dl_btn);

        popover.set_child(Some(&menu_box));

        let on_play_rc = Rc::new(on_play);
        let on_add_q_rc = Rc::new(on_add_queue);
        let on_toast_rc = Rc::new(on_toast);
        let on_open_channel_rc = Rc::new(on_open_channel);

        // Handlers
        let pop_weak = popover.downgrade();
        let vid_id = summary.id.clone();
        let vid_title = summary.title.clone();
        let on_play_cl = on_play_rc.clone();
        play_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_play_cl(vid_id.clone(), vid_title.clone(), Quality::P360);
        });

        let pop_weak = popover.downgrade();
        let summary_q = summary.clone();
        let on_add_q_cl = on_add_q_rc.clone();
        queue_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_add_q_cl(summary_q.clone());
        });

        let pop_weak = popover.downgrade();
        let on_toast_wl = on_toast_rc.clone();
        watch_later_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_toast_wl("Saved to Watch Later".to_string());
        });

        let pop_weak = popover.downgrade();
        let vid_id2 = summary.id.clone();
        let on_toast_cp = on_toast_rc.clone();
        copy_btn.connect_clicked(move |btn| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            let url = format!("https://www.youtube.com/watch?v={vid_id2}");
            let display = btn.display();
            display.clipboard().set_text(&url);
            on_toast_cp("Copied link to clipboard".to_string());
        });

        let pop_weak = popover.downgrade();
        let cid = summary.channel_id.clone();
        let cname = summary.channel_name.clone();
        let on_chan = on_open_channel_rc.clone();
        channel_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_chan(cid.clone(), cname.clone());
        });

        let pop_weak = popover.downgrade();
        let on_toast_dl = on_toast_rc.clone();
        dl_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            on_toast_dl("Download started in background".to_string());
        });

        // Gestures for right click & long press
        let right_click = GestureClick::new();
        right_click.set_button(3); // Secondary click
        let pop_weak = popover.downgrade();
        right_click.connect_pressed(move |_, _, x, y| {
            if let Some(pop) = pop_weak.upgrade() {
                let rect = Rectangle::new(x as i32, y as i32, 1, 1);
                pop.set_pointing_to(Some(&rect));
                pop.popup();
            }
        });
        target.add_controller(right_click);

        let long_press = GestureLongPress::new();
        let pop_weak2 = popover.downgrade();
        long_press.connect_pressed(move |_, x, y| {
            if let Some(pop) = pop_weak2.upgrade() {
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
