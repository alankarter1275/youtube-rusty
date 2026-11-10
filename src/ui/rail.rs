use crate::ui::icons::{make_icon, set_pointer, KalamIcon};
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Orientation, Overlay};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RailTab {
    Home,
    Subscriptions,
    Playlists,
    History,
    Settings,
}

pub struct Rail {
    pub container: Overlay,
    pub logo_btn: Button,
    pub home_btn: Button,
    pub subs_btn: Button,
    pub playlists_btn: Button,
    pub hist_btn: Button,
    pub settings_btn: Button,
}

impl Rail {
    pub fn new() -> Self {
        let base_box = GtkBox::new(Orientation::Vertical, 0);
        base_box.set_width_request(68);
        base_box.set_vexpand(true);
        base_box.set_hexpand(false);
        base_box.add_css_class("yr-rail");

        // Center Navigation Group: true vertical center of the full window height
        let center_box = GtkBox::new(Orientation::Vertical, 8);
        center_box.set_halign(Align::Center);
        center_box.set_valign(Align::Center);
        center_box.set_vexpand(true);

        let home_btn = create_nav_btn(KalamIcon::Home, "Home", true);
        let subs_btn = create_nav_btn(KalamIcon::Subscriptions, "Subscriptions", false);
        let playlists_btn = create_nav_btn(KalamIcon::Playlists, "Playlists", false);
        let hist_btn = create_nav_btn(KalamIcon::History, "History", false);

        center_box.append(&home_btn);
        center_box.append(&subs_btn);
        center_box.append(&playlists_btn);
        center_box.append(&hist_btn);
        base_box.append(&center_box);

        let overlay = Overlay::new();
        overlay.set_vexpand(true);
        overlay.set_hexpand(false);
        overlay.set_child(Some(&base_box));

        // Top App Logo: 38x38 terracotta badge pinned 24px from top
        let logo_btn = Button::new();
        logo_btn.set_child(Some(&make_icon(KalamIcon::LogoPlay, 38)));
        logo_btn.add_css_class("yr-logo");
        logo_btn.set_halign(Align::Center);
        logo_btn.set_valign(Align::Start);
        logo_btn.set_margin_top(24);
        logo_btn.set_tooltip_text(Some("youtube-rusty — Go Home"));
        set_pointer(&logo_btn);
        overlay.add_overlay(&logo_btn);

        // Bottom Settings Button: pinned 24px from bottom
        let settings_btn = create_nav_btn(KalamIcon::Settings, "Preferences", false);
        settings_btn.set_halign(Align::Center);
        settings_btn.set_valign(Align::End);
        settings_btn.set_margin_bottom(24);
        overlay.add_overlay(&settings_btn);

        // Symmetric Floating Right Divider Line (24px top, 24px bottom)
        let sep = GtkBox::new(Orientation::Vertical, 0);
        sep.add_css_class("yr-rail-sep");
        sep.set_halign(Align::End);
        sep.set_vexpand(true);
        sep.set_width_request(2);
        sep.set_margin_top(24);
        sep.set_margin_bottom(24);
        sep.set_can_target(false);
        overlay.add_overlay(&sep);

        Self {
            container: overlay,
            logo_btn,
            home_btn,
            subs_btn,
            playlists_btn,
            hist_btn,
            settings_btn,
        }
    }

    pub fn set_active(&self, active: Option<RailTab>) {
        self.home_btn.remove_css_class("active");
        self.subs_btn.remove_css_class("active");
        self.playlists_btn.remove_css_class("active");
        self.hist_btn.remove_css_class("active");
        self.settings_btn.remove_css_class("active");

        if let Some(tab) = active {
            match tab {
                RailTab::Home => self.home_btn.add_css_class("active"),
                RailTab::Subscriptions => self.subs_btn.add_css_class("active"),
                RailTab::Playlists => self.playlists_btn.add_css_class("active"),
                RailTab::History => self.hist_btn.add_css_class("active"),
                RailTab::Settings => self.settings_btn.add_css_class("active"),
            }
        }
    }
}

fn create_nav_btn(icon: KalamIcon, tooltip: &str, is_active: bool) -> Button {
    let btn = Button::new();
    btn.set_child(Some(&make_icon(icon, 22)));
    btn.add_css_class("yr-rail-btn");
    btn.set_halign(Align::Center);
    btn.set_width_request(44);
    btn.set_height_request(44);
    btn.set_tooltip_text(Some(tooltip));
    set_pointer(&btn);
    if is_active {
        btn.add_css_class("active");
    }
    btn
}
