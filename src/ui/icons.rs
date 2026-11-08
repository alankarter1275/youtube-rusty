use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{Align, DrawingArea, Widget};
use std::f64::consts::PI;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KalamIcon {
    LogoPlay,
    Home,
    Subscriptions,
    History,
    Settings,
    Menu,
    PlayFilled,
}

pub fn make_icon(kind: KalamIcon, size: i32) -> DrawingArea {
    let da = DrawingArea::builder()
        .content_width(size)
        .content_height(size)
        .halign(Align::Center)
        .valign(Align::Center)
        .can_target(false)
        .build();

    da.set_draw_func(move |area, cr, width, height| {
        let color = area.color();
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            color.alpha() as f64,
        );

        let w = width.max(1) as f64;
        let h = height.max(1) as f64;

        match kind {
            KalamIcon::LogoPlay => {
                // Exact 38x38 Kalam logo play triangle: M15 11.5 l12 7.5 -12 7.5 z
                let sx = w / 38.0;
                let sy = h / 38.0;
                cr.scale(sx, sy);
                cr.move_to(15.0, 11.5);
                cr.line_to(27.0, 19.0);
                cr.line_to(15.0, 26.5);
                cr.close_path();
                let _ = cr.fill();
            }
            KalamIcon::PlayFilled => {
                let sx = w / 24.0;
                let sy = h / 24.0;
                cr.scale(sx, sy);
                cr.move_to(8.5, 6.0);
                cr.line_to(18.5, 12.0);
                cr.line_to(8.5, 18.0);
                cr.close_path();
                let _ = cr.fill();
            }
            KalamIcon::Home => {
                setup_stroke(cr, w, h);
                // M3 11 l9 -8 9 8
                cr.move_to(3.0, 11.0);
                cr.line_to(12.0, 3.0);
                cr.line_to(21.0, 11.0);
                let _ = cr.stroke();
                // M5 10 v10 h14 V10
                cr.move_to(5.0, 10.0);
                cr.line_to(5.0, 20.0);
                cr.line_to(19.0, 20.0);
                cr.line_to(19.0, 10.0);
                let _ = cr.stroke();
            }
            KalamIcon::Subscriptions => {
                setup_stroke(cr, w, h);
                // Top stacked bar: M6 4 h12
                cr.move_to(6.0, 4.0);
                cr.line_to(18.0, 4.0);
                let _ = cr.stroke();
                // Rounded rect x=3 y=8 w=18 h=12 rx=2
                rounded_rect(cr, 3.0, 8.0, 18.0, 12.0, 2.0);
                let _ = cr.stroke();
                // Play triangle inside: M10 11.5 l5 2.5 -5 2.5 z
                cr.move_to(10.0, 11.4);
                cr.line_to(14.8, 14.0);
                cr.line_to(10.0, 16.6);
                cr.close_path();
                let _ = cr.fill();
            }
            KalamIcon::History => {
                setup_stroke(cr, w, h);
                // Circular rewind arc: center (12,12), r=8.5 from ~215 deg around to 150 deg
                cr.arc(12.0, 12.0, 8.5, -0.78 * PI, 0.88 * PI);
                let _ = cr.stroke();
                // Arrowhead at top-left: M3.2 4.2 v4.6 h4.6
                cr.move_to(3.2, 4.0);
                cr.line_to(3.2, 8.6);
                cr.line_to(7.8, 8.6);
                let _ = cr.stroke();
                // Clock hands: M12 7.2 v4.8 l3.2 2.0
                cr.move_to(12.0, 7.2);
                cr.line_to(12.0, 12.0);
                cr.line_to(15.2, 14.0);
                let _ = cr.stroke();
            }
            KalamIcon::Settings => {
                setup_stroke(cr, w, h);
                // Upper slider: M4 7 h9.5, circle (16,7,2.2), M18.5 7 h1.8
                cr.move_to(4.0, 7.0);
                cr.line_to(13.5, 7.0);
                let _ = cr.stroke();
                cr.arc(16.0, 7.0, 2.2, 0.0, 2.0 * PI);
                let _ = cr.stroke();
                cr.move_to(18.4, 7.0);
                cr.line_to(20.0, 7.0);
                let _ = cr.stroke();

                // Lower slider: M4 17 h1.6, circle (8,17,2.2), M10.5 17 h9.5
                cr.move_to(4.0, 17.0);
                cr.line_to(5.6, 17.0);
                let _ = cr.stroke();
                cr.arc(8.0, 17.0, 2.2, 0.0, 2.0 * PI);
                let _ = cr.stroke();
                cr.move_to(10.5, 17.0);
                cr.line_to(20.0, 17.0);
                let _ = cr.stroke();
            }
            KalamIcon::Menu => {
                setup_stroke(cr, w, h);
                for y in [6.0, 12.0, 18.0] {
                    cr.move_to(4.0, y);
                    cr.line_to(20.0, y);
                    let _ = cr.stroke();
                }
            }
        }
    });

    da
}

fn setup_stroke(cr: &cairo::Context, w: f64, h: f64) {
    cr.scale(w / 24.0, h / 24.0);
    cr.set_line_width(2.0);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

pub fn set_pointer(widget: &impl IsA<Widget>) {
    widget.set_cursor_from_name(Some("pointer"));
}
