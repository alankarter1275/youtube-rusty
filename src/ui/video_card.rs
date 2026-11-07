use crate::backend::models::VideoSummary;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label, Orientation, Overlay, Picture};

pub struct VideoCard {
    pub container: gtk4::FlowBoxChild,
    pub picture: Picture,
}

impl VideoCard {
    /// Grid View Card (for Home & Trending): fixed 320x180 16:9 thumbnail on top, title & channel below
    pub fn new_grid(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 8);
        root_box.set_width_request(320);
        root_box.add_css_class("grid-card");

        // 16:9 Picture Container
        let picture = Picture::new();
        picture.set_width_request(320);
        picture.set_height_request(180);
        picture.set_can_shrink(false);
        picture.set_content_fit(gtk4::ContentFit::Cover);
        picture.add_css_class("video-thumb");

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_css_class("thumb-container");

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("duration-badge");
            duration_label.set_halign(gtk4::Align::End);
            duration_label.set_valign(gtk4::Align::End);
            overlay.add_overlay(&duration_label);
        }

        // Details Box
        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_lines(2);
        title_label.set_max_width_chars(32);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(gtk4::Align::Start);
        title_label.add_css_class("video-title");

        let meta_text = format!("{} • {}", summary.channel_name, summary.view_count_text);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.set_halign(gtk4::Align::Start);
        meta_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        meta_label.add_css_class("video-meta");

        root_box.append(&overlay);
        root_box.append(&title_label);
        root_box.append(&meta_label);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));

        Self { container, picture }
    }

    /// Search View Card (YouTube Search horizontal layout): Thumbnail left (360x202), details right
    pub fn new_search(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Horizontal, 18);
        root_box.set_hexpand(true);
        root_box.add_css_class("search-card");

        // Left: 16:9 Thumbnail Overlay
        let picture = Picture::new();
        picture.set_width_request(360);
        picture.set_height_request(202);
        picture.set_can_shrink(false);
        picture.set_content_fit(gtk4::ContentFit::Cover);
        picture.add_css_class("video-thumb");

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_css_class("thumb-container");

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("duration-badge");
            duration_label.set_halign(gtk4::Align::End);
            duration_label.set_valign(gtk4::Align::End);
            overlay.add_overlay(&duration_label);
        }

        // Right: Information Column
        let info_box = GtkBox::new(Orientation::Vertical, 6);
        info_box.set_hexpand(true);
        info_box.set_valign(gtk4::Align::Start);

        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(gtk4::Align::Start);
        title_label.add_css_class("video-title");

        let meta_text = format!("{} • {}", summary.channel_name, summary.view_count_text);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.set_halign(gtk4::Align::Start);
        meta_label.add_css_class("video-meta");

        let desc_label = Label::new(Some(&summary.description_snippet));
        desc_label.set_wrap(true);
        desc_label.set_lines(2);
        desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        desc_label.set_halign(gtk4::Align::Start);
        desc_label.add_css_class("video-description");

        info_box.append(&title_label);
        info_box.append(&meta_label);
        if !summary.description_snippet.is_empty() {
            info_box.append(&desc_label);
        }

        root_box.append(&overlay);
        root_box.append(&info_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));

        Self { container, picture }
    }
}
