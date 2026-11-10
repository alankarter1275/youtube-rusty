use crate::backend::history::HistoryEntry;
use crate::backend::models::{ChannelSearchSummary, PlaylistSearchSummary, VideoSummary};
use crate::backend::playlists::SavedPlaylist;
use crate::ui::icons::{make_icon, set_pointer, KalamIcon};
use adw::prelude::*;
use gtk4::{
    gdk, Align, Box as GtkBox, Button, DragSource, DropTarget, EventControllerKey,
    EventSequenceState, GestureClick, GestureLongPress, Label, Orientation, Overflow, Overlay,
    Picture, PropagationPhase,
};
use libadwaita as adw;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListCardLayout {
    Wide,
    Compact,
    Stacked,
}

pub struct VideoCard {
    pub container: gtk4::FlowBoxChild,
    pub highlight_target: GtkBox,
    pub click_target: GtkBox,
    pub channel_target: Option<GtkBox>,
    pub picture: Picture,
    pub avatar: adw::Avatar,
}

impl VideoCard {
    /// Grid View Card (Kalam Spec)
    pub fn new_grid(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 10);
        root_box.set_width_request(132);
        root_box.set_hexpand(true);
        root_box.set_valign(Align::Start);
        root_box.add_css_class("yr-card");
        set_pointer(&root_box);

        let (picture, overlay) = make_thumb_overlay(&summary.duration_text);
        overlay.set_hexpand(true);
        overlay.set_height_request(164);

        let thumb_v_clamp = adw::Clamp::builder()
            .orientation(Orientation::Vertical)
            .maximum_size(164)
            .tightening_threshold(164)
            .hexpand(true)
            .child(&overlay)
            .build();

        let details_box = GtkBox::new(Orientation::Horizontal, 10);
        details_box.set_valign(Align::Start);

        let avatar_label = if !summary.channel_name.is_empty() {
            &summary.channel_name
        } else {
            &summary.title
        };
        let avatar = adw::Avatar::new(36, Some(avatar_label), true);
        avatar.set_valign(Align::Start);

        let avatar_wrap = GtkBox::new(Orientation::Vertical, 0);
        avatar_wrap.set_valign(Align::Start);
        avatar_wrap.append(&avatar);
        let channel_target = if !summary.channel_id.is_empty() {
            avatar_wrap.set_tooltip_text(Some(&format!(
                "Open channel: {}",
                if summary.channel_name.is_empty() {
                    "Channel"
                } else {
                    &summary.channel_name
                }
            )));
            set_pointer(&avatar_wrap);
            Some(avatar_wrap.clone())
        } else {
            None
        };

        let text_col = GtkBox::new(Orientation::Vertical, 3);
        text_col.set_hexpand(true);

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.add_css_class("yr-title");

        let meta_text = join_non_empty(&[
            &summary.channel_name,
            &summary.view_count_text,
            &summary.published_text,
        ]);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.set_halign(Align::Start);
        meta_label.set_xalign(0.0);
        meta_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        meta_label.add_css_class("yr-meta");

        text_col.append(&title_label);
        if !meta_text.is_empty() {
            text_col.append(&meta_label);
        }

        details_box.append(&avatar_wrap);
        details_box.append(&text_col);

        root_box.append(&thumb_v_clamp);
        root_box.append(&details_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));

        Self {
            container,
            highlight_target: root_box.clone(),
            click_target: root_box,
            channel_target,
            picture,
            avatar,
        }
    }

    /// Static Loading Skeleton Card (zero continuous frame-clock animation)
    pub fn new_skeleton() -> gtk4::FlowBoxChild {
        let root_box = GtkBox::new(Orientation::Vertical, 10);
        root_box.set_width_request(132);
        root_box.set_hexpand(true);
        root_box.set_valign(Align::Start);
        root_box.add_css_class("yr-card");

        let thumb_box = GtkBox::new(Orientation::Vertical, 0);
        thumb_box.set_hexpand(true);
        thumb_box.set_height_request(164);
        thumb_box.add_css_class("yr-thumb");
        thumb_box.add_css_class("yr-skeleton");

        let thumb_v_clamp = adw::Clamp::builder()
            .orientation(Orientation::Vertical)
            .maximum_size(164)
            .tightening_threshold(164)
            .hexpand(true)
            .child(&thumb_box)
            .build();

        let details_box = GtkBox::new(Orientation::Horizontal, 10);

        let avatar_box = GtkBox::new(Orientation::Vertical, 0);
        avatar_box.set_size_request(36, 36);
        avatar_box.set_valign(Align::Start);
        avatar_box.add_css_class("yr-skeleton");
        avatar_box.add_css_class("yr-skeleton-circle");

        let text_col = GtkBox::new(Orientation::Vertical, 6);
        text_col.set_hexpand(true);

        let line1 = GtkBox::new(Orientation::Horizontal, 0);
        line1.set_height_request(14);
        line1.set_hexpand(true);
        line1.add_css_class("yr-skeleton");

        let line2 = GtkBox::new(Orientation::Horizontal, 0);
        line2.set_height_request(12);
        line2.set_width_request(84);
        line2.set_halign(Align::Start);
        line2.add_css_class("yr-skeleton");

        text_col.append(&line1);
        text_col.append(&line2);

        details_box.append(&avatar_box);
        details_box.append(&text_col);

        root_box.append(&thumb_v_clamp);
        root_box.append(&details_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));
        container
    }

    /// Responsive Search Result Card
    pub fn new_search(summary: &VideoSummary, layout: ListCardLayout) -> Self {
        if layout == ListCardLayout::Stacked {
            return Self::new_grid(summary);
        }

        let (thumb_w, thumb_h, gap) = match layout {
            ListCardLayout::Wide => (300, 169, 18),
            ListCardLayout::Compact => (208, 117, 14),
            ListCardLayout::Stacked => (208, 117, 14),
        };

        let root_box = GtkBox::new(Orientation::Horizontal, gap);
        root_box.set_hexpand(true);
        root_box.add_css_class("search-card");
        set_pointer(&root_box);

        let badge_text = if summary.is_short {
            if !summary.duration_text.is_empty()
                && !summary.duration_text.eq_ignore_ascii_case("SHORTS")
            {
                format!("SHORTS · {}", summary.duration_text)
            } else {
                "SHORTS".to_string()
            }
        } else {
            summary.duration_text.clone()
        };
        let (picture, overlay) = make_thumb_overlay(&badge_text);
        overlay.set_size_request(thumb_w, thumb_h);
        let thumb_clamped = clamp_fixed_size(&overlay, thumb_w, thumb_h);

        let info_box = GtkBox::new(Orientation::Vertical, 5);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Start);
        info_box.set_margin_top(2);

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.add_css_class(if layout == ListCardLayout::Wide {
            "yr-search-title"
        } else {
            "yr-title"
        });
        info_box.append(&title_label);

        let stats_text = join_non_empty(&[&summary.view_count_text, &summary.published_text]);
        if !stats_text.is_empty() {
            let stats_label = Label::new(Some(&stats_text));
            stats_label.add_css_class("yr-meta");
            stats_label.set_halign(Align::Start);
            stats_label.set_xalign(0.0);
            stats_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            info_box.append(&stats_label);
        }

        let avatar_label = if !summary.channel_name.is_empty() {
            &summary.channel_name
        } else {
            &summary.title
        };
        let avatar = adw::Avatar::new(24, Some(avatar_label), true);
        avatar.set_valign(Align::Center);

        let mut channel_target = None;
        if !summary.channel_name.is_empty() {
            let channel_row = GtkBox::new(Orientation::Horizontal, 8);
            channel_row.set_halign(Align::Start);
            channel_row.set_valign(Align::Center);
            channel_row.set_margin_top(2);
            channel_row.set_margin_bottom(2);

            let channel_label = Label::new(Some(&summary.channel_name));
            channel_label.add_css_class("yr-meta");
            if !summary.channel_id.is_empty() {
                channel_label.add_css_class("yr-chan-hover");
                channel_row.set_tooltip_text(Some(&format!("Open {}", summary.channel_name)));
                set_pointer(&channel_row);
                channel_target = Some(channel_row.clone());
            }
            channel_label.set_halign(Align::Start);
            channel_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            channel_row.append(&avatar);
            channel_row.append(&channel_label);
            info_box.append(&channel_row);
        }

        if !summary.description_snippet.is_empty() {
            let desc_label = Label::new(Some(&summary.description_snippet));
            desc_label.set_wrap(true);
            desc_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
            desc_label.set_lines(if layout == ListCardLayout::Wide { 2 } else { 1 });
            desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            desc_label.set_halign(Align::Start);
            desc_label.set_xalign(0.0);
            desc_label.add_css_class("video-description");
            info_box.append(&desc_label);
        }

        root_box.append(&thumb_clamped);
        root_box.append(&info_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_child(Some(&root_box));
        container.set_hexpand(true);

        Self {
            container,
            highlight_target: root_box.clone(),
            click_target: root_box,
            channel_target,
            picture,
            avatar,
        }
    }

    /// Responsive Search Skeleton Card
    pub fn new_search_skeleton(layout: ListCardLayout) -> gtk4::FlowBoxChild {
        if layout == ListCardLayout::Stacked {
            return Self::new_skeleton();
        }

        let (thumb_w, thumb_h, gap) = match layout {
            ListCardLayout::Wide => (300, 169, 18),
            ListCardLayout::Compact | ListCardLayout::Stacked => (208, 117, 14),
        };

        let root_box = GtkBox::new(Orientation::Horizontal, gap);
        root_box.set_hexpand(true);
        root_box.add_css_class("search-card");

        let thumb_box = GtkBox::new(Orientation::Vertical, 0);
        thumb_box.set_size_request(thumb_w, thumb_h);
        thumb_box.add_css_class("yr-thumb");
        thumb_box.add_css_class("yr-skeleton");
        let thumb_clamped = clamp_fixed_size(&thumb_box, thumb_w, thumb_h);

        let info_box = GtkBox::new(Orientation::Vertical, 10);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Start);
        info_box.set_margin_top(6);

        let line_title = GtkBox::new(Orientation::Horizontal, 0);
        line_title.set_height_request(16);
        line_title.set_hexpand(true);
        line_title.add_css_class("yr-skeleton");

        let ch_row = GtkBox::new(Orientation::Horizontal, 8);
        let ch_av = GtkBox::new(Orientation::Vertical, 0);
        ch_av.set_size_request(24, 24);
        ch_av.add_css_class("yr-skeleton");
        ch_av.add_css_class("yr-skeleton-circle");
        let ch_txt = GtkBox::new(Orientation::Horizontal, 0);
        ch_txt.set_size_request(96, 13);
        ch_txt.add_css_class("yr-skeleton");
        ch_row.append(&ch_av);
        ch_row.append(&ch_txt);

        let line_desc = GtkBox::new(Orientation::Horizontal, 0);
        line_desc.set_height_request(13);
        line_desc.set_hexpand(true);
        line_desc.add_css_class("yr-skeleton");

        info_box.append(&line_title);
        info_box.append(&ch_row);
        info_box.append(&line_desc);

        root_box.append(&thumb_clamped);
        root_box.append(&info_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_child(Some(&root_box));
        container.set_hexpand(true);
        container
    }

    /// History Row Card
    pub fn new_history(
        entry: &HistoryEntry,
        layout: ListCardLayout,
        on_remove: impl Fn() + 'static,
    ) -> Self {
        let (thumb_w, thumb_h, gap) = match layout {
            ListCardLayout::Wide => (200, 112, 16),
            ListCardLayout::Compact => (168, 94, 14),
            ListCardLayout::Stacked => (124, 70, 10),
        };

        let root_box = GtkBox::new(Orientation::Horizontal, 8);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-history-row");

        let click_target = GtkBox::new(Orientation::Horizontal, gap);
        click_target.set_hexpand(true);
        set_pointer(&click_target);

        let (picture, overlay) = make_thumb_overlay(&entry.video.duration_text);
        overlay.set_size_request(thumb_w, thumb_h);
        let thumb_clamped = clamp_fixed_size(&overlay, thumb_w, thumb_h);

        let info_box = GtkBox::new(Orientation::Vertical, 3);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Center);

        let title_label = Label::new(Some(&entry.video.title));
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.add_css_class("yr-title");

        let meta_text = join_non_empty(&[&entry.video.channel_name, &entry.video.view_count_text]);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.add_css_class("yr-meta");
        meta_label.set_halign(Align::Start);
        meta_label.set_xalign(0.0);
        meta_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let watched_text = format_watched_label(entry.watched_at);
        let watched_label = Label::new(Some(&watched_text));
        watched_label.add_css_class("yr-meta");
        watched_label.set_halign(Align::Start);
        watched_label.set_xalign(0.0);
        watched_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        info_box.append(&title_label);
        if !meta_text.is_empty() {
            info_box.append(&meta_label);
        }
        info_box.append(&watched_label);

        click_target.append(&thumb_clamped);
        click_target.append(&info_box);

        let remove_btn = Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove from history")
            .valign(Align::Center)
            .build();
        remove_btn.add_css_class("flat");
        remove_btn.add_css_class("circular");
        set_pointer(&remove_btn);
        remove_btn.connect_clicked(move |_| {
            on_remove();
        });

        root_box.append(&click_target);
        root_box.append(&remove_btn);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_child(Some(&root_box));
        container.set_hexpand(true);

        let avatar = adw::Avatar::new(24, Some(&entry.video.channel_name), true);

        Self {
            container,
            highlight_target: root_box,
            click_target,
            channel_target: None,
            picture,
            avatar,
        }
    }

    /// Playlist Track Row (supports editing controls on saved playlists)
    #[allow(clippy::too_many_arguments)]
    pub fn new_playlist_track(
        video: &VideoSummary,
        index: usize,
        total: usize,
        layout: ListCardLayout,
        editable: bool,
        on_reorder: impl Fn(usize, usize) + 'static,
        on_remove: impl Fn() + 'static,
    ) -> Self {
        let (thumb_w, thumb_h, gap) = match layout {
            ListCardLayout::Wide => (184, 104, 14),
            ListCardLayout::Compact => (156, 88, 12),
            ListCardLayout::Stacked => (120, 68, 10),
        };

        let root_box = GtkBox::new(Orientation::Horizontal, 8);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-history-row");
        root_box.add_css_class("yr-playlist-row");

        let click_target = GtkBox::new(Orientation::Horizontal, gap);
        click_target.set_hexpand(true);
        set_pointer(&click_target);

        let idx_lbl = Label::new(Some(&format!("{}", index + 1)));
        idx_lbl.add_css_class("yr-meta");
        idx_lbl.set_width_chars(2);
        idx_lbl.set_valign(Align::Center);
        click_target.append(&idx_lbl);

        let (picture, overlay) = make_thumb_overlay(&video.duration_text);
        overlay.set_size_request(thumb_w, thumb_h);
        let thumb_clamped = clamp_fixed_size(&overlay, thumb_w, thumb_h);
        click_target.append(&thumb_clamped);

        let info_box = GtkBox::new(Orientation::Vertical, 3);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Center);

        let title_label = Label::new(Some(&video.title));
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.add_css_class("yr-title");
        info_box.append(&title_label);

        let meta_text = join_non_empty(&[&video.channel_name, &video.view_count_text]);
        if !meta_text.is_empty() {
            let meta_label = Label::new(Some(&meta_text));
            meta_label.add_css_class("yr-meta");
            meta_label.set_halign(Align::Start);
            meta_label.set_xalign(0.0);
            meta_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            info_box.append(&meta_label);
        }

        click_target.append(&info_box);

        if editable {
            let reorder: Rc<dyn Fn(usize, usize)> = Rc::new(on_reorder);
            let drag_handle = Button::builder()
                .icon_name("list-drag-handle-symbolic")
                .tooltip_text("Drag to reorder · Alt+Up/Down also works")
                .valign(Align::Center)
                .build();
            drag_handle.add_css_class("flat");
            drag_handle.add_css_class("yr-playlist-drag-handle");
            set_pointer(&drag_handle);
            root_box.append(&drag_handle);

            let drag_source = DragSource::new();
            drag_source.set_actions(gdk::DragAction::MOVE);
            let drag_index = index as u32;
            drag_source.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(&drag_index.to_value()))
            });
            drag_handle.add_controller(drag_source);

            let key_controller = EventControllerKey::new();
            let reorder_key = Rc::clone(&reorder);
            key_controller.connect_key_pressed(move |_, key, _, modifiers| {
                if !modifiers.contains(gdk::ModifierType::ALT_MASK) {
                    return gtk4::glib::Propagation::Proceed;
                }
                let target = if key == gdk::Key::Up && index > 0 {
                    Some(index - 1)
                } else if key == gdk::Key::Down && index + 1 < total {
                    Some(index + 1)
                } else {
                    None
                };
                if let Some(target) = target {
                    reorder_key(index, target);
                    gtk4::glib::Propagation::Stop
                } else {
                    gtk4::glib::Propagation::Proceed
                }
            });
            drag_handle.add_controller(key_controller);

            let drop_target = DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
            let row_enter = root_box.clone();
            drop_target.connect_enter(move |_, _, _| {
                row_enter.add_css_class("drop-target");
                gdk::DragAction::MOVE
            });
            let row_leave = root_box.clone();
            drop_target.connect_leave(move |_| {
                row_leave.remove_css_class("drop-target");
            });
            let row_drop = root_box.clone();
            let reorder_drop = Rc::clone(&reorder);
            drop_target.connect_drop(move |_, value, _, _| {
                row_drop.remove_css_class("drop-target");
                if let Ok(from) = value.get::<u32>() {
                    let from = from as usize;
                    if from < total && from != index {
                        reorder_drop(from, index);
                        return true;
                    }
                }
                false
            });
            root_box.add_controller(drop_target);
        }

        root_box.append(&click_target);

        if editable {
            let controls = GtkBox::new(Orientation::Horizontal, 2);
            controls.set_valign(Align::Center);

            let rm_btn = Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove from playlist")
                .build();
            rm_btn.add_css_class("flat");
            rm_btn.add_css_class("circular");
            set_pointer(&rm_btn);
            rm_btn.connect_clicked(move |_| on_remove());

            controls.append(&rm_btn);
            root_box.append(&controls);
        }

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_child(Some(&root_box));
        container.set_hexpand(true);

        let avatar = adw::Avatar::new(24, Some(&video.channel_name), true);

        Self {
            container,
            highlight_target: root_box,
            click_target,
            channel_target: None,
            picture,
            avatar,
        }
    }

    /// Shorts Card: 9:16 aspect ratio vertical card
    pub fn new_short(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 8);
        root_box.set_width_request(96);
        root_box.set_hexpand(true);
        root_box.set_valign(Align::Start);
        root_box.add_css_class("yr-card");
        set_pointer(&root_box);

        let (picture, overlay) = make_thumb_overlay(&summary.duration_text);
        overlay.set_hexpand(true);
        overlay.set_height_request(280);

        let thumb_v_clamp = adw::Clamp::builder()
            .orientation(Orientation::Vertical)
            .maximum_size(280)
            .tightening_threshold(280)
            .hexpand(true)
            .child(&overlay)
            .build();

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.add_css_class("yr-title");

        let stats_text = join_non_empty(&[&summary.view_count_text, &summary.published_text]);
        let views_label = Label::new(Some(&stats_text));
        views_label.set_halign(Align::Start);
        views_label.set_xalign(0.0);
        views_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        views_label.add_css_class("yr-meta");

        root_box.append(&thumb_v_clamp);
        root_box.append(&title_label);

        let channel_target = if !summary.channel_name.is_empty() && !summary.channel_id.is_empty() {
            let ch_box = GtkBox::new(Orientation::Horizontal, 4);
            ch_box.set_halign(Align::Start);
            set_pointer(&ch_box);
            let ch_label = Label::new(Some(&summary.channel_name));
            ch_label.set_halign(Align::Start);
            ch_label.set_xalign(0.0);
            ch_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            ch_label.add_css_class("yr-meta");
            ch_label.add_css_class("yr-channel-link");
            ch_box.append(&ch_label);
            root_box.append(&ch_box);
            Some(ch_box)
        } else if !summary.channel_name.is_empty() {
            let ch_label = Label::new(Some(&summary.channel_name));
            ch_label.set_halign(Align::Start);
            ch_label.set_xalign(0.0);
            ch_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            ch_label.add_css_class("yr-meta");
            root_box.append(&ch_label);
            None
        } else {
            None
        };

        if !stats_text.is_empty() {
            root_box.append(&views_label);
        }

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));

        let avatar = adw::Avatar::new(24, Some(&summary.channel_name), true);

        Self {
            container,
            highlight_target: root_box.clone(),
            click_target: root_box,
            channel_target,
            picture,
            avatar,
        }
    }

    pub fn new_short_skeleton() -> gtk4::FlowBoxChild {
        let root_box = GtkBox::new(Orientation::Vertical, 8);
        root_box.set_width_request(96);
        root_box.set_hexpand(true);
        root_box.set_valign(Align::Start);
        root_box.add_css_class("yr-card");

        let thumb_box = GtkBox::new(Orientation::Vertical, 0);
        thumb_box.set_hexpand(true);
        thumb_box.set_height_request(280);
        thumb_box.add_css_class("yr-thumb");
        thumb_box.add_css_class("yr-skeleton");

        let thumb_v_clamp = adw::Clamp::builder()
            .orientation(Orientation::Vertical)
            .maximum_size(280)
            .tightening_threshold(280)
            .hexpand(true)
            .child(&thumb_box)
            .build();

        let line1 = GtkBox::new(Orientation::Vertical, 0);
        line1.set_height_request(14);
        line1.set_hexpand(true);
        line1.add_css_class("yr-skeleton");
        line1.add_css_class("yr-skeleton-line");

        let line2 = GtkBox::new(Orientation::Vertical, 0);
        line2.set_height_request(12);
        line2.set_width_request(76);
        line2.set_halign(Align::Start);
        line2.add_css_class("yr-skeleton");
        line2.add_css_class("yr-skeleton-line");

        root_box.append(&thumb_v_clamp);
        root_box.append(&line1);
        root_box.append(&line2);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));
        container
    }

    /// Unified mouse interaction model:
    /// - Single left-click (`n_press == 1`): selects/highlights card ONLY
    /// - Double left-click (`n_press == 2`) or middle-click: plays immediately in MPV
    /// - Right-click (`button == 3`) or touch long-press: opens Video Details popup
    pub fn connect_card_interactions(
        &self,
        on_select: impl Fn(&GtkBox) + 'static,
        on_double_play: impl Fn() + 'static,
        on_right_details: impl Fn() + 'static,
    ) {
        let on_select_rc = Rc::new(on_select);
        let on_play_rc = Rc::new(on_double_play);
        let on_details_rc = Rc::new(on_right_details);

        // Button 1: single-click highlights, double-click plays in MPV
        let left_click = GestureClick::new();
        left_click.set_button(1);
        let hl_left = self.highlight_target.clone();
        let sel_left = on_select_rc.clone();
        let play_left = on_play_rc.clone();
        left_click.connect_pressed(move |gesture, n_press, _, _| {
            sel_left(&hl_left);
            if n_press == 2 {
                gesture.set_state(EventSequenceState::Claimed);
                play_left();
            }
        });
        self.click_target.add_controller(left_click);

        // Button 2 (middle-click): play immediately in MPV
        let mid_click = GestureClick::new();
        mid_click.set_button(2);
        let hl_mid = self.highlight_target.clone();
        let sel_mid = on_select_rc.clone();
        let play_mid = on_play_rc;
        mid_click.connect_released(move |gesture, _, _, _| {
            gesture.set_state(EventSequenceState::Claimed);
            sel_mid(&hl_mid);
            play_mid();
        });
        self.click_target.add_controller(mid_click);

        // Button 3 (right-click): select + open DetailsSheet popup
        let right_click = GestureClick::new();
        right_click.set_button(3);
        let hl_right = self.highlight_target.clone();
        let sel_right = on_select_rc.clone();
        let det_right = on_details_rc.clone();
        right_click.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(EventSequenceState::Claimed);
            sel_right(&hl_right);
            det_right();
        });
        self.click_target.add_controller(right_click);

        // Long-press (touchscreens): open DetailsSheet popup
        let long_press = GestureLongPress::new();
        let hl_lp = self.highlight_target.clone();
        let sel_lp = on_select_rc;
        let det_lp = on_details_rc;
        long_press.connect_pressed(move |gesture, _, _| {
            gesture.set_state(EventSequenceState::Claimed);
            sel_lp(&hl_lp);
            det_lp();
        });
        self.click_target.add_controller(long_press);
    }

    /// Direct click on avatar / channel name navigates straight to the channel page.
    pub fn connect_channel_click(&self, on_channel: impl Fn() + 'static) {
        if let Some(ref target) = self.channel_target {
            let click = GestureClick::new();
            click.set_button(1);
            click.set_propagation_phase(PropagationPhase::Capture);
            click.connect_released(move |gesture, _, _, _| {
                gesture.set_state(EventSequenceState::Claimed);
                on_channel();
            });
            target.add_controller(click);
        }
    }
}

pub struct ChannelResultCard {
    pub container: gtk4::FlowBoxChild,
    pub avatar: adw::Avatar,
}

impl ChannelResultCard {
    pub fn new(
        channel: &ChannelSearchSummary,
        layout: ListCardLayout,
        initially_subscribed: bool,
        on_select: impl Fn(&GtkBox) + 'static,
        on_open: impl Fn() + 'static,
        on_toggle_sub: impl Fn() -> bool + 'static,
    ) -> Self {
        let av_size = if layout == ListCardLayout::Stacked { 52 } else { 72 };
        let root_box = GtkBox::new(Orientation::Horizontal, 16);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-entity-card");

        let click_area = GtkBox::new(Orientation::Horizontal, 16);
        click_area.set_hexpand(true);
        click_area.set_valign(Align::Center);
        set_pointer(&click_area);

        let avatar = adw::Avatar::new(av_size, Some(&channel.name), true);
        avatar.set_valign(Align::Center);
        click_area.append(&avatar);

        let text_col = GtkBox::new(Orientation::Vertical, 4);
        text_col.set_hexpand(true);
        text_col.set_valign(Align::Center);

        let name_lbl = Label::new(Some(&channel.name));
        name_lbl.add_css_class("yr-search-title");
        name_lbl.set_halign(Align::Start);
        name_lbl.set_xalign(0.0);
        name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        text_col.append(&name_lbl);

        let meta_txt = join_non_empty(&[&channel.handle, &channel.subscriber_count_text]);
        if !meta_txt.is_empty() {
            let meta_lbl = Label::new(Some(&meta_txt));
            meta_lbl.add_css_class("yr-meta");
            meta_lbl.set_halign(Align::Start);
            meta_lbl.set_xalign(0.0);
            meta_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            text_col.append(&meta_lbl);
        }

        if !channel.description.is_empty() && layout != ListCardLayout::Stacked {
            let desc_lbl = Label::new(Some(&channel.description));
            desc_lbl.add_css_class("video-description");
            desc_lbl.set_halign(Align::Start);
            desc_lbl.set_xalign(0.0);
            desc_lbl.set_lines(2);
            desc_lbl.set_wrap(true);
            desc_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            text_col.append(&desc_lbl);
        }

        click_area.append(&text_col);

        let sub_btn = Button::with_label(if initially_subscribed {
            "Subscribed"
        } else {
            "Subscribe"
        });
        sub_btn.add_css_class("yr-btn");
        if !initially_subscribed {
            sub_btn.add_css_class("primary");
        }
        sub_btn.set_valign(Align::Center);
        set_pointer(&sub_btn);

        sub_btn.connect_clicked(move |btn| {
            let subbed = on_toggle_sub();
            if subbed {
                btn.set_label("Subscribed");
                btn.remove_css_class("primary");
            } else {
                btn.set_label("Subscribe");
                btn.add_css_class("primary");
            }
        });

        let open_btn = Button::with_label("Open");
        open_btn.add_css_class("yr-btn");
        open_btn.set_valign(Align::Center);
        set_pointer(&open_btn);

        let on_open_rc = Rc::new(on_open);
        let open_cb = on_open_rc.clone();
        open_btn.connect_clicked(move |_| open_cb());

        let click = GestureClick::new();
        click.set_button(1);
        let hl = root_box.clone();
        click.connect_pressed(move |gesture, n_press, _, _| {
            on_select(&hl);
            if n_press == 2 {
                gesture.set_state(EventSequenceState::Claimed);
                on_open_rc();
            }
        });
        click_area.add_controller(click);

        root_box.append(&click_area);
        if layout != ListCardLayout::Stacked {
            root_box.append(&open_btn);
        }
        root_box.append(&sub_btn);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));

        Self { container, avatar }
    }
}

pub struct PlaylistResultCard {
    pub container: gtk4::FlowBoxChild,
    pub picture: Picture,
}

impl PlaylistResultCard {
    pub fn new(
        playlist: &PlaylistSearchSummary,
        layout: ListCardLayout,
        initially_saved: bool,
        on_select: impl Fn(&GtkBox) + 'static,
        on_open: impl Fn() + 'static,
        on_save: impl Fn() + 'static,
    ) -> Self {
        let (thumb_w, thumb_h, gap) = match layout {
            ListCardLayout::Wide => (260, 146, 18),
            ListCardLayout::Compact => (196, 110, 14),
            ListCardLayout::Stacked => (144, 81, 12),
        };

        let root_box = GtkBox::new(Orientation::Horizontal, gap);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-entity-card");

        let click_area = GtkBox::new(Orientation::Horizontal, gap);
        click_area.set_hexpand(true);
        set_pointer(&click_area);

        let (picture, overlay) = make_thumb_overlay(&playlist.video_count_text);
        overlay.set_size_request(thumb_w, thumb_h);
        let thumb_clamped = clamp_fixed_size(&overlay, thumb_w, thumb_h);
        click_area.append(&thumb_clamped);

        let info_col = GtkBox::new(Orientation::Vertical, 6);
        info_col.set_hexpand(true);
        info_col.set_valign(Align::Center);

        let title_lbl = Label::new(Some(&playlist.name));
        title_lbl.add_css_class("yr-search-title");
        title_lbl.set_wrap(true);
        title_lbl.set_lines(2);
        title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_lbl.set_halign(Align::Start);
        title_lbl.set_xalign(0.0);
        info_col.append(&title_lbl);

        let sub_txt = join_non_empty(&[&playlist.channel_name, &playlist.video_count_text]);
        if !sub_txt.is_empty() {
            let meta_lbl = Label::new(Some(&sub_txt));
            meta_lbl.add_css_class("yr-meta");
            meta_lbl.set_halign(Align::Start);
            meta_lbl.set_xalign(0.0);
            meta_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            info_col.append(&meta_lbl);
        }

        let btn_row = GtkBox::new(Orientation::Horizontal, 8);
        btn_row.set_margin_top(4);

        let open_btn = Button::with_label("Open playlist");
        open_btn.add_css_class("yr-btn");
        open_btn.add_css_class("primary");
        set_pointer(&open_btn);

        let save_btn = Button::with_label(if initially_saved {
            "Saved ✓"
        } else {
            "Save playlist"
        });
        save_btn.add_css_class("yr-btn");
        set_pointer(&save_btn);

        let on_open_rc = Rc::new(on_open);
        let open_cb = on_open_rc.clone();
        open_btn.connect_clicked(move |_| open_cb());

        save_btn.connect_clicked(move |btn| {
            btn.set_label("Saved ✓");
            on_save();
        });

        btn_row.append(&open_btn);
        btn_row.append(&save_btn);
        info_col.append(&btn_row);

        click_area.append(&info_col);

        let click = GestureClick::new();
        click.set_button(1);
        let hl = root_box.clone();
        click.connect_pressed(move |gesture, n_press, _, _| {
            on_select(&hl);
            if n_press == 2 {
                gesture.set_state(EventSequenceState::Claimed);
                on_open_rc();
            }
        });
        click_area.add_controller(click);

        root_box.append(&click_area);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));

        Self { container, picture }
    }
}

pub struct SavedPlaylistCard {
    pub container: gtk4::FlowBoxChild,
    pub picture: Picture,
}

impl SavedPlaylistCard {
    pub fn new(
        playlist: &SavedPlaylist,
        on_select: impl Fn(&GtkBox) + 'static,
        on_open: impl Fn() + 'static,
        on_play_all: impl Fn() + 'static,
        on_delete: impl Fn() + 'static,
    ) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 10);
        root_box.set_width_request(132);
        root_box.set_hexpand(true);
        root_box.set_valign(Align::Start);
        root_box.add_css_class("yr-playlist-card");
        root_box.add_css_class("yr-card");

        let click_area = GtkBox::new(Orientation::Vertical, 8);
        click_area.set_hexpand(true);
        set_pointer(&click_area);

        let count = playlist.videos.len();
        let badge_txt = if count == 1 {
            "1 video".to_string()
        } else {
            format!("{count} videos")
        };

        let (picture, overlay) = make_thumb_overlay(&badge_txt);
        overlay.set_hexpand(true);
        overlay.set_height_request(156);

        let thumb_v_clamp = adw::Clamp::builder()
            .orientation(Orientation::Vertical)
            .maximum_size(156)
            .tightening_threshold(156)
            .hexpand(true)
            .child(&overlay)
            .build();
        click_area.append(&thumb_v_clamp);

        let title_lbl = Label::new(Some(&playlist.name));
        title_lbl.add_css_class("yr-title");
        title_lbl.set_halign(Align::Start);
        title_lbl.set_xalign(0.0);
        title_lbl.set_lines(2);
        title_lbl.set_wrap(true);
        title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        click_area.append(&title_lbl);

        let sub_txt = if !playlist.author.is_empty() {
            format!("{} · {badge_txt}", playlist.author)
        } else {
            badge_txt
        };
        let meta_lbl = Label::new(Some(&sub_txt));
        meta_lbl.add_css_class("yr-meta");
        meta_lbl.set_halign(Align::Start);
        meta_lbl.set_xalign(0.0);
        meta_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        click_area.append(&meta_lbl);

        let on_open_rc = Rc::new(on_open);
        let click = GestureClick::new();
        click.set_button(1);
        let hl = root_box.clone();
        let open_dbl = on_open_rc.clone();
        click.connect_pressed(move |gesture, n_press, _, _| {
            on_select(&hl);
            if n_press == 2 {
                gesture.set_state(EventSequenceState::Claimed);
                open_dbl();
            }
        });
        click_area.add_controller(click);

        let actions_row = GtkBox::new(Orientation::Horizontal, 6);
        let open_btn = Button::with_label("Open");
        open_btn.add_css_class("yr-btn");
        open_btn.add_css_class("yr-btn-compact");
        open_btn.set_hexpand(true);
        set_pointer(&open_btn);
        let open_cb = on_open_rc;
        open_btn.connect_clicked(move |_| open_cb());

        let play_btn = Button::with_label("Play");
        play_btn.add_css_class("yr-btn");
        play_btn.add_css_class("yr-btn-compact");
        play_btn.add_css_class("primary");
        play_btn.set_sensitive(!playlist.videos.is_empty());
        set_pointer(&play_btn);
        play_btn.connect_clicked(move |_| on_play_all());

        let del_btn = Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Delete playlist")
            .build();
        del_btn.add_css_class("flat");
        del_btn.add_css_class("circular");
        set_pointer(&del_btn);
        del_btn.connect_clicked(move |_| on_delete());

        actions_row.append(&open_btn);
        actions_row.append(&play_btn);
        actions_row.append(&del_btn);

        root_box.append(&click_area);
        root_box.append(&actions_row);

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));

        Self { container, picture }
    }
}

fn make_thumb_overlay(duration_text: &str) -> (Picture, Overlay) {
    let picture = Picture::new();
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_content_fit(gtk4::ContentFit::Cover);

    let overlay = Overlay::new();
    overlay.set_child(Some(&picture));
    overlay.set_overflow(Overflow::Hidden);
    overlay.add_css_class("yr-thumb");

    let play_badge = GtkBox::new(Orientation::Vertical, 0);
    play_badge.add_css_class("yr-play-badge");
    play_badge.set_halign(Align::Center);
    play_badge.set_valign(Align::Center);
    play_badge.set_can_target(false);
    let icon = make_icon(KalamIcon::PlayFilled, 18);
    icon.set_vexpand(true);
    icon.set_hexpand(true);
    play_badge.append(&icon);
    overlay.add_overlay(&play_badge);

    if !duration_text.is_empty() {
        let duration_label = Label::new(Some(duration_text));
        duration_label.add_css_class("yr-duration");
        duration_label.set_halign(Align::End);
        duration_label.set_valign(Align::End);
        duration_label.set_can_target(false);
        overlay.add_overlay(&duration_label);
    }

    (picture, overlay)
}

fn clamp_fixed_size(widget: &impl IsA<gtk4::Widget>, width: i32, height: i32) -> adw::Clamp {
    let v_clamp = adw::Clamp::builder()
        .orientation(Orientation::Vertical)
        .maximum_size(height)
        .tightening_threshold(height)
        .child(widget)
        .build();
    adw::Clamp::builder()
        .orientation(Orientation::Horizontal)
        .maximum_size(width)
        .tightening_threshold(width)
        .halign(Align::Start)
        .valign(Align::Start)
        .hexpand(false)
        .child(&v_clamp)
        .build()
}

fn join_non_empty(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn format_watched_label(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let diff = now.saturating_sub(timestamp);
    if diff < 60 {
        "Watched just now".to_string()
    } else if diff < 3600 {
        format!("Watched {}m ago", diff / 60)
    } else if diff < 86400 {
        "Watched today".to_string()
    } else if diff < 172_800 {
        "Watched yesterday".to_string()
    } else {
        format!("Watched {}d ago", diff / 86400)
    }
}
