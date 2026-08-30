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
    // The window starts as the banner — its own size, no title bar — and grows
    // into the interface once the checks are done.
    //
    // It is `resizable(true)` from the very first frame, on purpose: winit's
    // Wayland backend does not reliably honour a later
    // `ViewportCommand::Resizable(true)` sent to a window that was *created*
    // non-resizable (open issues against winit's Wayland backend; GNOME under
    // Wayland reproduces it every time), which is what left the grown window
    // stuck unresizable. Pinning `min == max == splash::SIZE` locks the banner
    // in place just as well as `resizable(false)` would have, but through a
    // size-hint change instead of the flag — and toggling a size hint at
    // runtime works everywhere. `become_main_window` relaxes both bounds once
    // the checks are done.
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(splash::SIZE)
            .with_min_inner_size(splash::SIZE)
            .with_max_inner_size(splash::SIZE)
            .with_resizable(true)
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
