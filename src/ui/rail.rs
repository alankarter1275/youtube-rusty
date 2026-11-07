use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Image, Orientation, Overlay};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailTab {
    Home,
    Subscriptions,
    History,
    Settings,
}

pub struct Rail {
    pub container: Overlay,
    pub home_btn: Button,
    pub subs_btn: Button,
    pub hist_btn: Button,
    pub settings_btn: Button,
}

impl Rail {
    pub fn new() -> Self {
        let rail_box = GtkBox::new(Orientation::Vertical, 0);
        rail_box.add_css_class("yr-rail");
        rail_box.set_width_request(68);

        // Logo at top: 28px from top (terracotta rounded square with play icon)
        let logo_box = GtkBox::new(Orientation::Vertical, 0);
        logo_box.set_halign(Align::Center);
        logo_box.set_valign(Align::Start);
        logo_box.add_css_class("yr-logo");
        let logo_icon = Image::from_icon_name("media-playback-start-symbolic");
        logo_icon.set_pixel_size(18);
        logo_box.append(&logo_icon);
        rail_box.append(&logo_box);

        // Center navigation group: Home, Subscriptions, History
        let center_box = GtkBox::new(Orientation::Vertical, 16);
        center_box.set_valign(Align::Center);
        center_box.set_vexpand(true);
        center_box.set_halign(Align::Center);

        let home_btn = create_rail_button("user-home-symbolic", "Home", true);
        let subs_btn = create_rail_button("emblem-favorite-symbolic", "Subscriptions", false);
        let hist_btn = create_rail_button("document-open-recent-symbolic", "History", false);

        center_box.append(&home_btn);
        center_box.append(&subs_btn);
        center_box.append(&hist_btn);
        rail_box.append(&center_box);

        // Settings button pinned to bottom (24px bottom inset)
        let settings_btn = create_rail_button("emblem-system-symbolic", "Settings", false);
        settings_btn.set_valign(Align::End);
        settings_btn.set_halign(Align::Center);
        rail_box.append(&settings_btn);

        // Inset 2px separator line at the rail's right edge
        let sep = GtkBox::new(Orientation::Vertical, 0);
        sep.add_css_class("yr-rail-sep");
        sep.set_width_request(2);
        sep.set_halign(Align::End);

        let container = Overlay::new();
        container.set_child(Some(&rail_box));
        container.add_overlay(&sep);

        Self {
            container,
            home_btn,
            subs_btn,
            hist_btn,
            settings_btn,
        }
    }

    pub fn set_active(&self, tab: RailTab) {
        self.home_btn.remove_css_class("active");
        self.subs_btn.remove_css_class("active");
        self.hist_btn.remove_css_class("active");
        self.settings_btn.remove_css_class("active");

        match tab {
            RailTab::Home => self.home_btn.add_css_class("active"),
            RailTab::Subscriptions => self.subs_btn.add_css_class("active"),
            RailTab::History => self.hist_btn.add_css_class("active"),
            RailTab::Settings => self.settings_btn.add_css_class("active"),
        }
    }
}

fn create_rail_button(icon_name: &str, tooltip: &str, is_active: bool) -> Button {
    let btn = Button::from_icon_name(icon_name);
    btn.add_css_class("yr-rail-btn");
    btn.set_tooltip_text(Some(tooltip));
    btn.set_width_request(44);
    btn.set_height_request(44);
    if is_active {
        btn.add_css_class("active");
    }
    btn
}
