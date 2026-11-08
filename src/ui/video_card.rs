use crate::backend::models::VideoSummary;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation, Overlay, Picture};
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
        let root_box = GtkBox::new(Orientation::Horizontal, 16);
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

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("yr-duration");
            duration_label.set_halign(Align::End);
            duration_label.set_valign(Align::End);
            overlay.add_overlay(&duration_label);
        }

        let info_box = GtkBox::new(Orientation::Vertical, 6);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Start);

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(Align::Start);
        title_label.add_css_class("yr-title");

        let channel_row = GtkBox::new(Orientation::Horizontal, 8);
        channel_row.set_valign(Align::Center);
        let avatar = adw::Avatar::new(24, Some(&summary.channel_name), true);
        let meta_text = format!("{} · {}", summary.channel_name, summary.view_count_text);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.add_css_class("yr-meta");
        channel_row.append(&avatar);
        channel_row.append(&meta_label);

        let desc_label = Label::new(Some(&summary.description_snippet));
        desc_label.set_wrap(true);
        desc_label.set_lines(2);
        desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        desc_label.set_halign(Align::Start);
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

        Self {
            container,
            picture,
            avatar,
            summary: summary.clone(),
        }
    }
}
