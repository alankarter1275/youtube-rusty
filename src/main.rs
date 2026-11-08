mod backend;
mod player;
mod ui;

use adw::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;
use ui::window::MainWindow;

const APP_ID: &str = "com.kunal.YouTubeRusty";

fn main() {
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
