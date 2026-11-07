use adw::prelude::*;
use gtk4::{Box as GtkBox, FlowBox, Orientation, ScrolledWindow, SearchEntry, SelectionMode};
use libadwaita as adw;

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub search_entry: SearchEntry,
    pub flow_box: FlowBox,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Self {
        // Enforce dark style matching system Wayland aesthetic
        let style_manager = adw::StyleManager::default();
        style_manager.set_color_scheme(adw::ColorScheme::PreferDark);

        // Header bar with embedded search entry
        let header_bar = adw::HeaderBar::new();
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search YouTube...")
            .width_request(380)
            .build();
        header_bar.set_title_widget(Some(&search_entry));

        // Video grid inside a smooth scrolled window
        let flow_box = FlowBox::builder()
            .selection_mode(SelectionMode::None)
            .homogeneous(false)
            .max_children_per_line(6)
            .min_children_per_line(1)
            .column_spacing(16)
            .row_spacing(16)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();

        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&flow_box)
            .build();

        // Main vertical content box
        let content_box = GtkBox::new(Orientation::Vertical, 0);
        content_box.append(&header_bar);
        content_box.append(&scrolled_window);

        // Application window
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("YouTube Rusty")
            .default_width(1150)
            .default_height(720)
            .content(&content_box)
            .build();

        Self {
            window,
            search_entry,
            flow_box,
        }
    }

    pub fn present(&self) {
        self.window.present();
    }
}
