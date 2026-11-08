use crate::backend::models::VideoSummary;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Overlay, Picture};
use libadwaita as adw;

pub struct VideoCard {
    pub container: gtk4::FlowBoxChild,
    pub picture: Picture,
    pub avatar: adw::Avatar,
    pub summary: VideoSummary,
}

impl VideoCard {
    /// Grid View Card (Phase 2 Kalam Spec):
    /// 16:9 thumbnail (radius 14, ContentFit::Cover), duration badge bottom-right,
    /// below it a 36px circular adw::Avatar, title (15px / 500, max 2 lines, ellipsis),
    /// one muted line `Channel · 19.8M views`.
    pub fn new_grid(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 10);
        root_box.set_width_request(280);
        root_box.add_css_class("yr-card");

        // 16:9 Thumbnail Container
        let picture = Picture::new();
        picture.set_width_request(280);
        picture.set_height_request(158);
        picture.set_can_shrink(false);
        picture.set_content_fit(gtk4::ContentFit::Cover);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_css_class("yr-thumb");

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("yr-duration");
            duration_label.set_halign(Align::End);
            duration_label.set_valign(Align::End);
            overlay.add_overlay(&duration_label);
        }

        // Details Row: [ 36px circular Avatar ] + [ Title + Channel · Views ]
        let details_box = GtkBox::new(Orientation::Horizontal, 12);
        details_box.set_valign(Align::Start);

        let avatar = adw::Avatar::new(36, Some(&summary.channel_name), true);
        avatar.set_valign(Align::Start);

        let text_col = GtkBox::new(Orientation::Vertical, 4);
        text_col.set_hexpand(true);

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.add_css_class("yr-title");

        let meta_text = format!("{} · {}", summary.channel_name, summary.view_count_text);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.set_halign(Align::Start);
        meta_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        meta_label.add_css_class("yr-meta");

        text_col.append(&title_label);
        text_col.append(&meta_label);

        details_box.append(&avatar);
        details_box.append(&text_col);

        root_box.append(&overlay);
        root_box.append(&details_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));

        Self {
            container,
            picture,
            avatar,
            summary: summary.clone(),
        }
    }

    /// Loading Skeleton Card (Phase 2 Spec: opacity pulse 900ms)
    pub fn new_skeleton() -> gtk4::FlowBoxChild {
        let root_box = GtkBox::new(Orientation::Vertical, 10);
        root_box.set_width_request(280);
        root_box.add_css_class("yr-card");

        // Skeleton Thumbnail
        let thumb_box = GtkBox::new(Orientation::Vertical, 0);
        thumb_box.set_width_request(280);
        thumb_box.set_height_request(158);
        thumb_box.add_css_class("yr-thumb");
        thumb_box.add_css_class("yr-skeleton");

        let details_box = GtkBox::new(Orientation::Horizontal, 12);
        let avatar_box = GtkBox::new(Orientation::Vertical, 0);
        avatar_box.set_width_request(36);
        avatar_box.set_height_request(36);
        avatar_box.add_css_class("yr-skeleton");
        avatar_box.set_valign(Align::Start);

        let text_col = GtkBox::new(Orientation::Vertical, 6);
        text_col.set_hexpand(true);

        let line1 = GtkBox::new(Orientation::Horizontal, 0);
        line1.set_height_request(14);
        line1.add_css_class("yr-skeleton");

        let line2 = GtkBox::new(Orientation::Horizontal, 0);
        line2.set_height_request(12);
        line2.set_width_request(140);
        line2.set_halign(Align::Start);
        line2.add_css_class("yr-skeleton");

        text_col.append(&line1);
        text_col.append(&line2);

        details_box.append(&avatar_box);
        details_box.append(&text_col);

        root_box.append(&thumb_box);
        root_box.append(&details_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));
        container
    }

    /// Horizontal Search Result Card
    pub fn new_search(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Horizontal, 18);
        root_box.set_hexpand(true);
        root_box.add_css_class("search-card");

        let picture = Picture::new();
        picture.set_width_request(300);
        picture.set_height_request(169);
        picture.set_can_shrink(false);
        picture.set_content_fit(gtk4::ContentFit::Cover);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_css_class("yr-thumb");
        overlay.set_size_request(300, 169);

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("yr-duration");
            duration_label.set_halign(Align::End);
            duration_label.set_valign(Align::End);
            overlay.add_overlay(&duration_label);
        }

        let info_box = GtkBox::new(Orientation::Vertical, 8);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Start);
        info_box.set_margin_top(4);

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.add_css_class("yr-title");

        let channel_row = GtkBox::new(Orientation::Horizontal, 8);
        channel_row.set_valign(Align::Center);
        let avatar = adw::Avatar::new(24, Some(&summary.channel_name), true);
        avatar.set_valign(Align::Center);

        let meta_text = format!("{} · {}", summary.channel_name, summary.view_count_text);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.add_css_class("yr-meta");
        meta_label.set_halign(Align::Start);
        channel_row.append(&avatar);
        channel_row.append(&meta_label);

        let desc_label = Label::new(Some(&summary.description_snippet));
        desc_label.set_wrap(true);
        desc_label.set_lines(2);
        desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        desc_label.set_halign(Align::Start);
        desc_label.set_xalign(0.0);
        desc_label.add_css_class("video-description");

        info_box.append(&title_label);
        info_box.append(&channel_row);
        if !summary.description_snippet.is_empty() {
            info_box.append(&desc_label);
        }

        root_box.append(&overlay);
        root_box.append(&info_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));
        container.set_hexpand(true);

        Self {
            container,
            picture,
            avatar,
            summary: summary.clone(),
        }
    }

    /// Horizontal Search Skeleton Card
    pub fn new_search_skeleton() -> gtk4::FlowBoxChild {
        let root_box = GtkBox::new(Orientation::Horizontal, 18);
        root_box.set_hexpand(true);
        root_box.add_css_class("search-card");

        let thumb_box = GtkBox::new(Orientation::Vertical, 0);
        thumb_box.set_size_request(300, 169);
        thumb_box.add_css_class("yr-thumb");
        thumb_box.add_css_class("yr-skeleton");

        let info_box = GtkBox::new(Orientation::Vertical, 10);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Start);
        info_box.set_margin_top(6);

        let line_title = GtkBox::new(Orientation::Horizontal, 0);
        line_title.set_height_request(18);
        line_title.set_width_request(380);
        line_title.set_halign(Align::Start);
        line_title.add_css_class("yr-skeleton");

        let ch_row = GtkBox::new(Orientation::Horizontal, 8);
        let ch_av = GtkBox::new(Orientation::Vertical, 0);
        ch_av.set_size_request(24, 24);
        ch_av.add_css_class("yr-skeleton");
        let ch_txt = GtkBox::new(Orientation::Horizontal, 0);
        ch_txt.set_size_request(160, 14);
        ch_txt.add_css_class("yr-skeleton");
        ch_row.append(&ch_av);
        ch_row.append(&ch_txt);

        let line_desc = GtkBox::new(Orientation::Horizontal, 0);
        line_desc.set_height_request(14);
        line_desc.set_width_request(450);
        line_desc.set_halign(Align::Start);
        line_desc.add_css_class("yr-skeleton");

        info_box.append(&line_title);
        info_box.append(&ch_row);
        info_box.append(&line_desc);

        root_box.append(&thumb_box);
        root_box.append(&info_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));
        container.set_hexpand(true);
        container
    }

    /// History Row Card
    pub fn new_history(
        entry: &crate::backend::history::HistoryEntry,
        on_remove: impl Fn() + 'static,
    ) -> (gtk4::FlowBoxChild, Picture) {
        let root_box = GtkBox::new(Orientation::Horizontal, 16);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-history-row");

        let picture = Picture::new();
        picture.set_width_request(200);
        picture.set_height_request(112);
        picture.set_can_shrink(false);
        picture.set_content_fit(gtk4::ContentFit::Cover);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_css_class("yr-thumb");
        overlay.set_size_request(200, 112);

        if !entry.video.duration_text.is_empty() {
            let duration_label = Label::new(Some(&entry.video.duration_text));
            duration_label.add_css_class("yr-duration");
            duration_label.set_halign(Align::End);
            duration_label.set_valign(Align::End);
            overlay.add_overlay(&duration_label);
        }

        let info_box = GtkBox::new(Orientation::Vertical, 6);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Center);

        let title_label = Label::new(Some(&entry.video.title));
        title_label.set_wrap(true);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.set_xalign(0.0);
        title_label.add_css_class("yr-title");

        let rel_time = format_relative_time(entry.watched_at);
        let meta_text = format!(
            "{} · {} · Watched {}",
            entry.video.channel_name, entry.video.view_count_text, rel_time
        );
        let meta_label = Label::new(Some(&meta_text));
        meta_label.add_css_class("yr-meta");
        meta_label.set_halign(Align::Start);

        info_box.append(&title_label);
        info_box.append(&meta_label);

        let remove_btn = Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove from history")
            .valign(Align::Center)
            .build();
        remove_btn.add_css_class("flat");
        remove_btn.connect_clicked(move |_| {
            on_remove();
        });

        root_box.append(&overlay);
        root_box.append(&info_box);
        root_box.append(&remove_btn);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));
        container.set_hexpand(true);

        (container, picture)
    }

    /// Shorts Card: 6-column 9:16 aspect ratio vertical card
    pub fn new_short(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 8);
        root_box.set_width_request(180);
        root_box.add_css_class("yr-card");

        let picture = Picture::new();
        picture.set_width_request(180);
        picture.set_height_request(320);
        picture.set_can_shrink(false);
        picture.set_content_fit(gtk4::ContentFit::Cover);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_css_class("yr-thumb");

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("yr-duration");
            duration_label.set_halign(Align::End);
            duration_label.set_valign(Align::End);
            overlay.add_overlay(&duration_label);
        }

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.add_css_class("yr-title");

        let views_label = Label::new(Some(&summary.view_count_text));
        views_label.set_halign(Align::Start);
        views_label.add_css_class("yr-meta");

        root_box.append(&overlay);
        root_box.append(&title_label);
        root_box.append(&views_label);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));

        let avatar = adw::Avatar::new(24, Some(&summary.channel_name), true);

        Self {
            container,
            picture,
            avatar,
            summary: summary.clone(),
        }
    }
}

fn format_relative_time(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let diff = now.saturating_sub(timestamp);
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 172800 {
        "yesterday".to_string()
    } else {
        format!("{}d ago", diff / 86400)
    }
}
