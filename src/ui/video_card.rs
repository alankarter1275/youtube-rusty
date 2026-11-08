use crate::backend::history::HistoryEntry;
use crate::backend::models::VideoSummary;
use adw::prelude::*;
use gtk4::{
    Align, AspectFrame, Box as GtkBox, Button, GestureClick, Label, Orientation, Overlay, Picture,
};
use libadwaita as adw;

pub struct VideoCard {
    pub container: gtk4::FlowBoxChild,
    pub click_target: GtkBox,
    pub picture: Picture,
    pub avatar: adw::Avatar,
}

impl VideoCard {
    /// Grid View Card (Kalam Spec):
    /// 16:9 thumbnail (radius 14, ContentFit::Cover), duration badge bottom-right,
    /// 36px circular adw::Avatar, title (max 2 lines), `Channel · views`.
    pub fn new_grid(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 10);
        root_box.set_width_request(140);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-card");

        let picture = Picture::new();
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_content_fit(gtk4::ContentFit::Cover);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        overlay.add_css_class("yr-thumb");

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("yr-duration");
            duration_label.set_halign(Align::End);
            duration_label.set_valign(Align::End);
            overlay.add_overlay(&duration_label);
        }

        let aspect = AspectFrame::new(0.5, 0.5, 16.0 / 9.0, false);
        aspect.set_hexpand(true);
        aspect.set_child(Some(&overlay));

        let details_box = GtkBox::new(Orientation::Horizontal, 12);
        details_box.set_valign(Align::Start);

        let avatar = adw::Avatar::new(36, Some(&summary.channel_name), true);
        avatar.set_valign(Align::Start);

        let text_col = GtkBox::new(Orientation::Vertical, 4);
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
        text_col.append(&meta_label);

        details_box.append(&avatar);
        details_box.append(&text_col);

        root_box.append(&aspect);
        root_box.append(&details_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_hexpand(true);
        container.set_valign(Align::Start);
        container.set_child(Some(&root_box));

        Self {
            container,
            click_target: root_box,
            picture,
            avatar,
        }
    }

    /// Loading Skeleton Card (opacity pulse 900ms)
    pub fn new_skeleton() -> gtk4::FlowBoxChild {
        let root_box = GtkBox::new(Orientation::Vertical, 10);
        root_box.set_width_request(140);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-card");

        let thumb_box = GtkBox::new(Orientation::Vertical, 0);
        thumb_box.set_hexpand(true);
        thumb_box.set_vexpand(true);
        thumb_box.add_css_class("yr-thumb");
        thumb_box.add_css_class("yr-skeleton");

        let aspect = AspectFrame::new(0.5, 0.5, 16.0 / 9.0, false);
        aspect.set_hexpand(true);
        aspect.set_child(Some(&thumb_box));

        let details_box = GtkBox::new(Orientation::Horizontal, 12);

        let avatar_box = GtkBox::new(Orientation::Vertical, 0);
        avatar_box.set_size_request(36, 36);
        avatar_box.set_valign(Align::Start);
        avatar_box.add_css_class("yr-skeleton");
        avatar_box.add_css_class("yr-skeleton-circle");

        let text_col = GtkBox::new(Orientation::Vertical, 6);
        text_col.set_hexpand(true);

        let line1 = GtkBox::new(Orientation::Horizontal, 0);
        line1.set_height_request(15);
        line1.set_hexpand(true);
        line1.add_css_class("yr-skeleton");

        let line2 = GtkBox::new(Orientation::Horizontal, 0);
        line2.set_height_request(13);
        line2.set_width_request(110);
        line2.set_halign(Align::Start);
        line2.add_css_class("yr-skeleton");

        text_col.append(&line1);
        text_col.append(&line2);

        details_box.append(&avatar_box);
        details_box.append(&text_col);

        root_box.append(&aspect);
        root_box.append(&details_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_hexpand(true);
        container.set_valign(Align::Start);
        container.set_child(Some(&root_box));
        container
    }

    /// Horizontal Search Result Card (matches Search results.png):
    /// 300px thumbnail left; right column has title (17px, 2 lines), `views · age`,
    /// channel row (24px avatar + name), 2-line description snippet.
    pub fn new_search(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Horizontal, 18);
        root_box.set_hexpand(true);
        root_box.add_css_class("search-card");

        let picture = Picture::new();
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
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

        let thumb_clamped = clamp_fixed_size(&overlay, 300, 169);

        let info_box = GtkBox::new(Orientation::Vertical, 6);
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
        title_label.add_css_class("yr-search-title");

        let stats_text = join_non_empty(&[&summary.view_count_text, &summary.published_text]);
        let stats_label = Label::new(Some(&stats_text));
        stats_label.add_css_class("yr-meta");
        stats_label.set_halign(Align::Start);
        stats_label.set_xalign(0.0);

        let channel_row = GtkBox::new(Orientation::Horizontal, 8);
        channel_row.set_valign(Align::Center);
        channel_row.set_margin_top(2);
        let avatar = adw::Avatar::new(24, Some(&summary.channel_name), true);
        avatar.set_valign(Align::Center);

        let channel_label = Label::new(Some(&summary.channel_name));
        channel_label.add_css_class("yr-meta");
        channel_label.set_halign(Align::Start);
        channel_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        channel_row.append(&avatar);
        channel_row.append(&channel_label);

        info_box.append(&title_label);
        if !stats_text.is_empty() {
            info_box.append(&stats_label);
        }
        info_box.append(&channel_row);

        if !summary.description_snippet.is_empty() {
            let desc_label = Label::new(Some(&summary.description_snippet));
            desc_label.set_wrap(true);
            desc_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
            desc_label.set_lines(2);
            desc_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            desc_label.set_halign(Align::Start);
            desc_label.set_xalign(0.0);
            desc_label.add_css_class("video-description");
            info_box.append(&desc_label);
        }

        root_box.append(&thumb_clamped);
        root_box.append(&info_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));
        container.set_hexpand(true);

        Self {
            container,
            click_target: root_box,
            picture,
            avatar,
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
        let thumb_clamped = clamp_fixed_size(&thumb_box, 300, 169);

        let info_box = GtkBox::new(Orientation::Vertical, 10);
        info_box.set_hexpand(true);
        info_box.set_valign(Align::Start);
        info_box.set_margin_top(6);

        let line_title = GtkBox::new(Orientation::Horizontal, 0);
        line_title.set_height_request(18);
        line_title.set_width_request(320);
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
        line_desc.set_width_request(380);
        line_desc.set_halign(Align::Start);
        line_desc.add_css_class("yr-skeleton");

        info_box.append(&line_title);
        info_box.append(&ch_row);
        info_box.append(&line_desc);

        root_box.append(&thumb_clamped);
        root_box.append(&info_box);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));
        container.set_hexpand(true);
        container
    }

    /// History Row Card (matches History.png):
    /// 200px thumbnail, title, `channel · views`, `Watched today`, and a remove (x) button.
    pub fn new_history(entry: &HistoryEntry, on_remove: impl Fn() + 'static) -> Self {
        let root_box = GtkBox::new(Orientation::Horizontal, 12);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-history-row");

        let click_target = GtkBox::new(Orientation::Horizontal, 16);
        click_target.set_hexpand(true);

        let picture = Picture::new();
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
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

        let thumb_clamped = clamp_fixed_size(&overlay, 200, 112);

        let info_box = GtkBox::new(Orientation::Vertical, 4);
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

        info_box.append(&title_label);
        info_box.append(&meta_label);
        info_box.append(&watched_label);

        click_target.append(&thumb_clamped);
        click_target.append(&info_box);

        let remove_btn = Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove from history")
            .valign(Align::Center)
            .build();
        remove_btn.add_css_class("flat");
        remove_btn.connect_clicked(move |_| {
            on_remove();
        });

        root_box.append(&click_target);
        root_box.append(&remove_btn);

        let container = gtk4::FlowBoxChild::new();
        container.set_child(Some(&root_box));
        container.set_hexpand(true);

        let avatar = adw::Avatar::new(24, Some(&entry.video.channel_name), true);

        Self {
            container,
            click_target,
            picture,
            avatar,
        }
    }

    /// Shorts Card: 6-column 9:16 aspect ratio vertical card
    pub fn new_short(summary: &VideoSummary) -> Self {
        let root_box = GtkBox::new(Orientation::Vertical, 8);
        root_box.set_width_request(100);
        root_box.set_hexpand(true);
        root_box.add_css_class("yr-card");

        let picture = Picture::new();
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_content_fit(gtk4::ContentFit::Cover);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        overlay.add_css_class("yr-thumb");

        if !summary.duration_text.is_empty() {
            let duration_label = Label::new(Some(&summary.duration_text));
            duration_label.add_css_class("yr-duration");
            duration_label.set_halign(Align::End);
            duration_label.set_valign(Align::End);
            overlay.add_overlay(&duration_label);
        }

        let aspect = AspectFrame::new(0.5, 0.5, 9.0 / 16.0, false);
        aspect.set_hexpand(true);
        aspect.set_child(Some(&overlay));

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

        root_box.append(&aspect);
        root_box.append(&title_label);
        root_box.append(&views_label);

        let container = gtk4::FlowBoxChild::new();
        container.set_hexpand(true);
        container.set_valign(Align::Start);
        container.set_child(Some(&root_box));

        let avatar = adw::Avatar::new(24, Some(&summary.channel_name), true);

        Self {
            container,
            click_target: root_box,
            picture,
            avatar,
        }
    }

    pub fn connect_primary_click(&self, on_click: impl Fn() + 'static) {
        let click = GestureClick::new();
        click.set_button(1);
        click.connect_released(move |_, _, _, _| {
            on_click();
        });
        self.click_target.add_controller(click);
    }
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
