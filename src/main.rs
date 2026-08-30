#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod boot;
mod job;
mod media;
mod preset;
mod splash;
mod theme;

/// The interface's size, once the start-up banner has stepped aside.
pub const WINDOW: [f32; 2] = [1180.0, 760.0];
pub const MIN_WINDOW: [f32; 2] = [620.0, 420.0];

fn main() -> eframe::Result<()> {
    // ffmpeg is not checked here: the banner opens straight away and reports
    // the check itself, so a missing ffmpeg is explained in the app instead of
    // on a terminal a desktop launcher never shows.
    //
    // The window starts as the banner — its own size, no title bar, not
    // resizable — and grows into the interface once the checks are done.
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(splash::SIZE)
            .with_resizable(false)
            .with_decorations(false)
            .with_title(splash::NAME)
            // Matches `StartupWMClass` in vidnux.desktop, so the desktop pairs
            // the window with the installed icon and app name, and draws the
            // title bar in the system theme (Adwaita on GNOME).
            .with_app_id("vidnux"),
        ..Default::default()
    };
    eframe::run_native(
        splash::NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
