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
                let on_add_queue = move |_sum: backend::models::VideoSummary| {};
                let on_open_channel = move |_cid: String, _cname: String| {};

                ui::details_sheet::DetailsSheet::show(
                    &win_clone.window,
                    &sample,
                    None,
                    None,
                    on_play,
                    on_add_queue,
                    on_toast,
                    on_open_channel,
                );
            });
        }

        if std::env::var("TEST_PLAYER").is_ok() {
            let win_clone = window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                let sample1 = backend::models::VideoSummary {
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
                    description_snippet: "".to_string(),
                };
                let sample2 = backend::models::VideoSummary {
                    id: "xyz123".to_string(),
                    title: "Traditional Bengal Silk Weaving Documentary".to_string(),
                    channel_id: "UC_crafts".to_string(),
                    channel_name: "Heritage Explorers".to_string(),
                    channel_avatar_url: "".to_string(),
                    duration_seconds: Some(920),
                    duration_text: "15:20".to_string(),
                    view_count_text: "800K views".to_string(),
                    published_text: "1 week ago".to_string(),
                    thumbnail_url: "".to_string(),
                    description_snippet: "".to_string(),
                };
                win_clone.player_card.set_queue(vec![sample1, sample2], 0);
                win_clone.player_card.set_progress(252.0, 742.0);
                win_clone.player_card.set_playing(true);
                win_clone.player_card.expand_btn.emit_clicked();
            });
        }

        if std::env::var("TEST_CHANNEL").is_ok() {
            let win_clone = window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                if let Some(cp) = win_clone.open_channel("UC_ancient_history", "Heritage Explorers") {
                    let details = backend::models::ChannelDetails {
                        id: "UC_ancient_history".to_string(),
                        name: "Heritage Explorers".to_string(),
                        handle: "@heritage_explorers".to_string(),
                        subscriber_count_text: "1.24M subscribers".to_string(),
                        video_count_text: "348 videos".to_string(),
                        avatar_url: "".to_string(),
                    };
                    let sample_vids = vec![
                        backend::models::VideoSummary {
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
                            description_snippet: "A deep dive into 17th-century terracotta architecture.".to_string(),
                        },
                        backend::models::VideoSummary {
                            id: "vid2".to_string(),
                            title: "Lost Cities of the Indus Civilization & Brick Making".to_string(),
                            channel_id: "UC_ancient_history".to_string(),
                            channel_name: "Heritage Explorers".to_string(),
                            channel_avatar_url: "".to_string(),
                            duration_seconds: Some(1120),
                            duration_text: "18:40".to_string(),
                            view_count_text: "920K views".to_string(),
                            published_text: "2 weeks ago".to_string(),
                            thumbnail_url: "".to_string(),
                            description_snippet: "Exploring Harappan kilns and structural engineering.".to_string(),
                        },
                    ];
                    let sample_shorts = vec![
                        backend::models::VideoSummary {
                            id: "short1".to_string(),
                            title: "Carving 400-Year-Old Terracotta Bricks in 60s #shorts".to_string(),
                            channel_id: "UC_ancient_history".to_string(),
                            channel_name: "Heritage Explorers".to_string(),
                            channel_avatar_url: "".to_string(),
                            duration_seconds: Some(58),
                            duration_text: "0:58".to_string(),
                            view_count_text: "3.2M views".to_string(),
                            published_text: "1 month ago".to_string(),
                            thumbnail_url: "".to_string(),
                            description_snippet: "".to_string(),
                        },
                        backend::models::VideoSummary {
                            id: "short2".to_string(),
                            title: "Secret Inscriptions on Temple Pillars #shorts".to_string(),
                            channel_id: "UC_ancient_history".to_string(),
                            channel_name: "Heritage Explorers".to_string(),
                            channel_avatar_url: "".to_string(),
                            duration_seconds: Some(42),
                            duration_text: "0:42".to_string(),
                            view_count_text: "1.8M views".to_string(),
                            published_text: "2 months ago".to_string(),
                            thumbnail_url: "".to_string(),
                            description_snippet: "".to_string(),
                        },
                    ];

                    cp.set_channel_details(details);
                    cp.set_videos(sample_vids);
                    cp.set_shorts(sample_shorts);

                    if std::env::var("TEST_CHANNEL_SHORTS").is_ok() {
                        cp.switch_tab(ui::channel_page::ChannelTab::Shorts);
                    }
                }
            });
        }
    });

    app.run();
}
