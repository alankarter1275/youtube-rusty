use crate::backend::models::VideoSummary;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Label, Orientation, Picture};

pub struct VideoCard {
    pub container: gtk4::FlowBoxChild,
    pub picture: Picture,
    pub video_id: String,
    pub title: String,
}

impl VideoCard {
    pub fn new(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 6);
        root_box.set_width_request(240);
        root_box.set_margin_bottom(8);
        root_box.add_css_class("card");

        // Thumbnail Picture
        let picture = Picture::new();
        picture.set_width_request(240);
        picture.set_height_request(135);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk4::ContentFit::Cover);
        picture.add_css_class("rounded-thumbnail");

        // Duration overlay badge simulation via box
        let duration_label = Label::new(Some(&summary.duration_text));
        duration_label.add_css_class("caption");
        duration_label.set_halign(gtk4::Align::End);
        duration_label.set_margin_end(6);

        // Title Label
        let title_label = Label::new(Some(&summary.title));
        title_label.set_wrap(true);
        title_label.set_max_width_chars(28);
        title_label.set_lines(2);
        title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_label.set_halign(gtk4::Align::Start);
        title_label.set_margin_start(6);
        title_label.set_margin_end(6);
        title_label.add_css_class("heading");

        // Channel and Views subtitle
        let meta_text = format!("{} • {}", summary.channel_name, summary.view_count_text);
        let meta_label = Label::new(Some(&meta_text));
        meta_label.set_halign(gtk4::Align::Start);
        meta_label.set_margin_start(6);
        meta_label.set_margin_end(6);
        meta_label.set_margin_bottom(6);
        meta_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        meta_label.add_css_class("dim-label");

        root_box.append(&picture);
        root_box.append(&duration_label);
        root_box.append(&title_label);
        root_box.append(&meta_label);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));

        Self {
            container,
            picture,
            video_id: summary.id.clone(),
            title: summary.title.clone(),
        }
    }
}
