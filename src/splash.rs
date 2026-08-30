//! The start-up banner, in the spirit of the one DaVinci Resolve shows: a wide
//! illustrated plate with the wordmark and version on the left, the author's
//! mark in the corner, and a line naming whatever is being checked right now,
//! so a slow `ffmpeg -encoders` never looks like a hung window.
//!
//! The banner has its own window size — [`SIZE`], undecorated — and the window
//! only grows into the interface once the checks are done. See
//! `App::become_main_window`.

use crate::boot::Progress;
use crate::theme::Palette;

/// Drawn behind everything. See `assets/splash.svg`, which is cut to exactly
/// these proportions.
const BACKGROUND: egui::ImageSource<'_> = egui::include_image!("../assets/splash.svg");

/// The author's mark, top right, where Resolve puts the Blackmagic logo.
const AUTHOR_MARK: egui::ImageSource<'_> = egui::include_image!("../assets/pandora_headshot.svg");

/// Banner window size, in points. Matches the artwork's aspect exactly so the
/// plate fills the window edge to edge with no letterboxing.
pub const SIZE: egui::Vec2 = egui::vec2(1000.0, 460.0);

pub const AUTHOR: &str = "dewakuneiei";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = "Vidnux";

/// Everything is drawn on the artwork rather than baked into it, so it stays
/// crisp at any scale and the loading line can change per frame.
pub fn show(ctx: &egui::Context, p: &Progress, pal: &Palette) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let plate = ui.max_rect();
            egui::Image::new(BACKGROUND).paint_at(ui, plate);

            let s = (plate.width() / SIZE.x).min(plate.height() / SIZE.y);
            let at = |x: f32, y: f32| plate.min + egui::vec2(x * s, y * s);

            author_mark(ui, plate, s);
            wordmark(ui, at(72.0, 150.0), s);

            match &p.error {
                Some(msg) => trouble(ui, at(72.0, 300.0), plate, s, msg, pal),
                None => loading(ui, at(72.0, 330.0), s, p, pal),
            }

            footer(ui, at(72.0, 412.0), s);
        });
}

/// `Vidnux` over its version, the way Resolve stacks its name over `20`.
fn wordmark(ui: &egui::Ui, at: egui::Pos2, s: f32) {
    let p = ui.painter();
    p.text(
        at,
        egui::Align2::LEFT_TOP,
        NAME,
        egui::FontId::proportional(58.0 * s),
        egui::Color32::from_rgb(240, 243, 250),
    );
    p.text(
        at + egui::vec2(3.0 * s, 74.0 * s),
        egui::Align2::LEFT_TOP,
        VERSION,
        egui::FontId::proportional(26.0 * s),
        egui::Color32::from_rgb(150, 160, 180),
    );
}

/// The author's headshot and name, top right.
fn author_mark(ui: &egui::Ui, plate: egui::Rect, s: f32) {
    let size = egui::vec2(54.0, 46.0) * s;
    let icon = egui::Rect::from_min_size(
        egui::pos2(plate.right() - 40.0 * s - size.x, plate.top() + 58.0 * s),
        size,
    );
    egui::Image::new(AUTHOR_MARK).paint_at(ui, icon);
    ui.painter().text(
        egui::pos2(icon.left() - 12.0 * s, icon.center().y),
        egui::Align2::RIGHT_CENTER,
        AUTHOR,
        egui::FontId::proportional(19.0 * s),
        egui::Color32::from_rgb(214, 220, 232),
    );
}

fn loading(ui: &egui::Ui, at: egui::Pos2, s: f32, p: &Progress, pal: &Palette) {
    let p_ = ui.painter();
    let track = egui::Rect::from_min_size(at, egui::vec2(320.0 * s, 3.0 * s));
    p_.rect_filled(track, 2.0, egui::Color32::from_rgb(48, 54, 68));
    let mut filled = track;
    filled.set_width(track.width() * p.fraction.clamp(0.0, 1.0));
    p_.rect_filled(filled, 2.0, pal.accent);

    p_.text(
        at + egui::vec2(0.0, 16.0 * s),
        egui::Align2::LEFT_TOP,
        format!("{}…", p.step),
        egui::FontId::proportional(14.0 * s),
        egui::Color32::from_rgb(178, 187, 204),
    );

    // The checks that already answered, so the wait shows its working.
    for (i, line) in p.log.iter().rev().take(2).enumerate() {
        p_.text(
            at + egui::vec2(0.0, (38.0 + 15.0 * i as f32) * s),
            egui::Align2::LEFT_TOP,
            line,
            egui::FontId::monospace(11.0 * s),
            egui::Color32::from_rgb(112, 120, 136),
        );
    }
}

/// No ffmpeg: say so on the banner and give the command, rather than failing on
/// a terminal a desktop launcher never shows.
fn trouble(ui: &mut egui::Ui, at: egui::Pos2, plate: egui::Rect, s: f32, msg: &str, pal: &Palette) {
    ui.painter().text(
        at,
        egui::Align2::LEFT_TOP,
        msg,
        egui::FontId::proportional(16.0 * s),
        pal.bad,
    );
    for (i, (distro, cmd)) in [
        ("Fedora / Nobara", "sudo dnf install ffmpeg"),
        ("Debian / Ubuntu", "sudo apt install ffmpeg"),
        ("Arch", "sudo pacman -S ffmpeg"),
    ]
    .into_iter()
    .enumerate()
    {
        let y = at.y + (26.0 + 16.0 * i as f32) * s;
        ui.painter().text(
            egui::pos2(at.x, y),
            egui::Align2::LEFT_TOP,
            format!("{distro:<17}{cmd}"),
            egui::FontId::monospace(11.5 * s),
            egui::Color32::from_rgb(150, 160, 180),
        );
    }

    let quit = egui::Rect::from_min_size(
        egui::pos2(plate.right() - 140.0 * s, plate.bottom() - 84.0 * s),
        egui::vec2(96.0 * s, 30.0 * s),
    );
    if ui
        .put(quit, egui::Button::new("Quit").corner_radius(6))
        .clicked()
    {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn footer(ui: &egui::Ui, at: egui::Pos2, s: f32) {
    ui.painter().text(
        at,
        egui::Align2::LEFT_TOP,
        format!("Created by {AUTHOR}  ·  MIT licensed  ·  powered by ffmpeg"),
        egui::FontId::proportional(11.5 * s),
        egui::Color32::from_rgb(120, 128, 145),
    );
}

#[cfg(test)]
mod tests {
    use super::{AUTHOR_MARK, BACKGROUND, SIZE, VERSION};

    /// The banner is the first thing anyone sees, and a broken SVG shows up as
    /// a placeholder rather than an error — so check both really rasterise.
    #[test]
    fn the_banner_artwork_loads() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);

        for art in [BACKGROUND, AUTHOR_MARK] {
            let egui::ImageSource::Bytes { uri, bytes } = art else {
                panic!("the artwork should be embedded in the binary");
            };
            ctx.include_bytes(uri.clone(), bytes);

            let mut loaded = false;
            for _ in 0..300 {
                match ctx.try_load_image(&uri, egui::SizeHint::Width(1000)) {
                    Ok(egui::load::ImagePoll::Ready { image }) => {
                        assert_eq!(image.width(), 1000, "{uri}");
                        assert!(image.height() > 0, "{uri}");
                        loaded = true;
                        break;
                    }
                    Ok(egui::load::ImagePoll::Pending { .. }) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(e) => panic!("{uri} did not load: {e}"),
                }
            }
            assert!(loaded, "{uri} never finished loading");
        }
    }

    #[test]
    fn the_banner_shows_a_real_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(VERSION.starts_with(char::is_numeric), "{VERSION}");
        // The artwork is cut to the window, so any letterboxing would be a bug.
        assert!((SIZE.x / SIZE.y - 1000.0 / 460.0).abs() < f32::EPSILON);
    }
}
