mod backend;
mod player;
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

        if std::env::var("TEST_DETAILS").is_ok() {
            let win_clone = window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                let sample = backend::models::VideoSummary {
                    id: "dQw4w9WgXcQ".to_string(),
                    title: "Exploring the Ancient Terracotta Temples of Bishnupur".to_string(),
                    channel_id: "UC_ancient_history".to_string(),
                    channel_name: "Heritage Explorers".to_string(),
                    channel_avatar_url: "".to_string(),
                    duration_seconds: Some(742),
                    duration_text: "12:22".to_string(),
                    view_count_text: "1.4M views".to_string(),
                    published_text: "3 days ago".to_string(),
                    thumbnail_url: "".to_string(),
                    description_snippet: "A deep dive into the 17th-century terracotta architecture and artistry in West Bengal.".to_string(),
                };
                let toast_overlay = win_clone.toast_overlay.clone();
                let on_toast = move |msg: String| {
                    toast_overlay.add_toast(adw::Toast::new(&msg));
                };
                let toast_overlay_play = win_clone.toast_overlay.clone();
                let on_play = move |_id: String, title: String, quality: player::mpv::Quality| {
                    let q_name = match quality {
                        player::mpv::Quality::P360 => "360p",
                        player::mpv::Quality::P720 => "720p",
                        player::mpv::Quality::P1080 => "1080p",
                        player::mpv::Quality::AudioOnly => "Audio",
                    };
                    toast_overlay_play.add_toast(adw::Toast::new(&format!(
                        "Playing \"{title}\" ({q_name}) in MPV..."
                    )));
                };
                let on_open_channel = move |_cid: String, _cname: String| {};

                ui::details_sheet::DetailsSheet::show(
                    &win_clone.window,
                    &sample,
                    None,
                    None,
                    on_play,
                    on_toast,
                    on_open_channel,
                );
            });
        }
    });

    app.run();
}
