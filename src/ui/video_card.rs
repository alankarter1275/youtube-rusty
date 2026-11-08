use crate::backend::history::HistoryEntry;
use crate::backend::models::VideoSummary;
use crate::ui::icons::{make_icon, set_pointer, KalamIcon};
use adw::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, EventSequenceState, GestureClick, Label, Orientation, Overflow,
    Overlay, Picture, PropagationPhase,
};
use libadwaita as adw;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListCardLayout {
    Wide,
    Compact,
    Stacked,
}

pub struct VideoCard {
    pub container: gtk4::FlowBoxChild,
    pub click_target: GtkBox,
    pub channel_target: Option<GtkBox>,
    pub picture: Picture,
    pub avatar: adw::Avatar,
}

impl VideoCard {
    /// Grid View Card (Kalam Spec):
    /// 16:9 thumbnail (radius 14, ContentFit::Cover), duration badge bottom-right,
    /// 36px circular adw::Avatar, title (max 2 lines), `Channel · views`.
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

        let meta_text = join_non_empty(&[&summary.channel_name, &summary.view_count_text]);
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
            click_target: root_box,
            channel_target,
            picture,
            avatar,
        }
    }

    /// Loading Skeleton Card (opacity pulse 900ms)
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

    /// Responsive Search Result Card:
    /// - Wide (>780px): 300x169 thumbnail left, rich metadata right (matches Search results.png)
    /// - Compact (601..=780px): 208x117 thumbnail left so half-screen tiled windows never overflow
    /// - Stacked (<=600px): vertical full-width card matching Narrow window.png
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

        let (picture, overlay) = make_thumb_overlay(&summary.duration_text);
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
            click_target: root_box,
            channel_target,
            picture,
            avatar,
        }
    }

    /// Responsive Search Skeleton Card (fluid width, zero fixed horizontal overflow)
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

    /// History Row Card (matches History.png, responsive down to narrow widths)
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

        let views_label = Label::new(Some(&summary.view_count_text));
        views_label.set_halign(Align::Start);
        views_label.set_xalign(0.0);
        views_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        views_label.add_css_class("yr-meta");

        root_box.append(&thumb_v_clamp);
        root_box.append(&title_label);
        if !summary.view_count_text.is_empty() {
            root_box.append(&views_label);
        }

        let container = gtk4::FlowBoxChild::new();
        container.set_focusable(false);
        container.set_hexpand(true);
        container.set_child(Some(&root_box));

        let avatar = adw::Avatar::new(24, Some(&summary.channel_name), true);

        Self {
            container,
            click_target: root_box,
            channel_target: None,
            picture,
            avatar,
        }
    }

    pub fn connect_primary_click(&self, on_click: impl Fn() + 'static) {
        let click = GestureClick::new();
        click.set_button(1);
        click.connect_released(move |gesture, _, _, _| {
            gesture.set_state(EventSequenceState::Claimed);
            on_click();
        });
        self.click_target.add_controller(click);
    }

    /// Middle-click on any card plays immediately in MPV at default quality.
    pub fn connect_middle_click(&self, on_middle: impl Fn() + 'static) {
        let click = GestureClick::new();
        click.set_button(2);
        click.connect_released(move |gesture, _, _, _| {
            gesture.set_state(EventSequenceState::Claimed);
            on_middle();
        });
        self.click_target.add_controller(click);
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

    // Subtle hover play badge in center
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
