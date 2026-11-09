mod backend;
mod player;
mod ui;

use adw::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;
use ui::window::MainWindow;

const APP_ID: &str = "com.kunal.YouTubeRusty";

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn configure_low_memory_allocator() {
    extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }
    const M_TRIM_THRESHOLD: i32 = -1;
    const M_MMAP_THRESHOLD: i32 = -3;
    const M_ARENA_MAX: i32 = -8;
    // Cap glibc arenas to 2 and pin mmap/trim thresholds at 128 KiB so decoded
    // RGBA buffers and large JSON payloads are returned to the OS immediately.
    unsafe {
        let _ = mallopt(M_ARENA_MAX, 2);
        let _ = mallopt(M_MMAP_THRESHOLD, 128 * 1024);
        let _ = mallopt(M_TRIM_THRESHOLD, 128 * 1024);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn configure_low_memory_allocator() {}

/// Silences a known benign GTK 4.18–4.20 Wayland frame-clock false-positive warning
/// (`Duplicate call to gdk_frame_timings_presented() ...`) emitted when an external window
/// such as MPV covers and uncovers the app surface (upstream GNOME/gtk MR !10242).
fn configure_glib_log_filter() {
    gtk4::glib::log_set_writer_func(|level, fields| {
        if level == gtk4::glib::LogLevel::Warning {
            let mut is_gdk = false;
            let mut is_benign_frame_timing = false;
            for f in fields {
                match f.key() {
                    "GLIB_DOMAIN" => {
                        if f.value_str() == Some("Gdk") {
                            is_gdk = true;
                        }
                    }
                    "MESSAGE" => {
                        if f.value_str()
                            .map(|m| {
                                m.starts_with("Duplicate call to gdk_frame_timings_presented()")
                            })
                            .unwrap_or(false)
                        {
                            is_benign_frame_timing = true;
                        }
                    }
                    _ => {}
                }
            }
            if is_gdk && is_benign_frame_timing {
                return gtk4::glib::LogWriterOutput::Handled;
            }
        }
        gtk4::glib::log_writer_default(level, fields)
    });
}

fn main() {
    configure_low_memory_allocator();
    configure_glib_log_filter();

    let app = adw::Application::builder().application_id(APP_ID).build();

    // Hold a single main-window handle tied to the application's lifetime
    // while widget signal handlers only capture Weak<MainWindow> references.
    let active_window: Rc<RefCell<Option<Rc<MainWindow>>>> = Rc::new(RefCell::new(None));

    let win_slot = active_window.clone();
    app.connect_activate(move |app| {
        if let Some(existing) = win_slot.borrow().as_ref() {
            existing.present();
            return;
        }

        let window = MainWindow::new(app);
        let slot_clear = win_slot.clone();
        window.window.connect_destroy(move |_| {
            *slot_clear.borrow_mut() = None;
        });

        window.present();
        *win_slot.borrow_mut() = Some(window);
    });

    app.run();
}
