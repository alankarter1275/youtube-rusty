mod ui;

use adw::prelude::*;
use libadwaita as adw;
use ui::window::MainWindow;

const APP_ID: &str = "com.kunal.YouTubeRusty";

fn main() {
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        let window = MainWindow::new(app);
        window.present();
    });

    app.run();
}
