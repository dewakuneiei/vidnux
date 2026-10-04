//! "Cover art": put a picture into a video file as its thumbnail — the image
//! file managers and players show instead of a random frame. The streams are
//! copied untouched, so nothing is re-encoded and it takes a moment.

use crate::media;
use crate::theme::Palette;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

const PREVIEW_W: usize = 320;
const PREVIEW_H: usize = 180;

pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];

#[derive(Clone, PartialEq)]
enum State {
    Idle,
    Working,
    Done(PathBuf),
    Failed(String),
}

/// A picture on its way to the screen: pixels first, texture once drawn.
#[derive(Default)]
struct Preview {
    pixels: Option<Arc<Vec<u8>>>,
    tex: Option<egui::TextureHandle>,
}

pub struct Cover {
    video: Option<PathBuf>,
    image: Option<PathBuf>,
    video_preview: Preview,
    image_preview: Preview,
    state: Arc<Mutex<State>>,
}

impl Cover {
    pub fn new() -> Self {
        Self {
            video: None,
            image: None,
            video_preview: Preview::default(),
            image_preview: Preview::default(),
            state: Arc::new(Mutex::new(State::Idle)),
        }
    }

    /// Files dropped on the window: a video fills the video slot, a picture
    /// fills the picture slot.
    pub fn accept(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for p in paths {
            if is_image(&p) {
                self.set_image(p, ctx);
            } else if crate::job::is_video(&p) {
                self.set_video(p, ctx);
            }
        }
    }

    fn set_video(&mut self, p: PathBuf, ctx: &egui::Context) {
        let at = media::probe(&p).map(|i| i.duration * 0.1).ok();
        self.video_preview = Preview::default();
        self.video_preview.pixels =
            media::frame(&p, at, PREVIEW_W, PREVIEW_H).map(Arc::new);
        self.video = Some(p);
        *self.state.lock().unwrap() = State::Idle;
        ctx.request_repaint();
    }

    fn set_image(&mut self, p: PathBuf, ctx: &egui::Context) {
        self.image_preview = Preview::default();
        self.image_preview.pixels = media::frame(&p, None, PREVIEW_W, PREVIEW_H).map(Arc::new);
        self.image = Some(p);
        *self.state.lock().unwrap() = State::Idle;
        ctx.request_repaint();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let state = self.state.lock().unwrap().clone();
        let ctx = ui.ctx().clone();

        ui.add_space(8.0);
        ui.heading("Cover art");
        ui.label(
            egui::RichText::new(
                "Pick a video and a picture. The picture becomes the video's thumbnail; \\
                 the video itself is copied as it is, with no quality loss.",
            )
            .weak(),
        );
        ui.add_space(12.0);

        ui.columns(2, |cols| {
            let picked = slot(
                &mut cols[0],
                pal,
                "1  Video",
                &mut self.video_preview,
                self.video.as_deref(),
                "Choose video…",
                "Drop a video here",
            );
            if picked {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Choose a video")
                    .add_filter("Video files", crate::job::VIDEO_EXTS)
                    .pick_file()
                {
                    self.set_video(p, &ctx);
                }
            }
            let picked = slot(
                &mut cols[1],
                pal,
                "2  Thumbnail picture",
                &mut self.image_preview,
                self.image.as_deref(),
                "Choose picture…",
                "Drop a JPG / PNG / WebP here",
            );
            if picked {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Choose a thumbnail picture")
                    .add_filter("Pictures", IMAGE_EXTS)
                    .pick_file()
                {
                    self.set_image(p, &ctx);
                }
            }
        });

        ui.add_space(14.0);

        let unsupported = self
            .video
            .as_deref()
            .filter(|v| !supports_cover(v))
            .map(|v| ext_of(v));
        if let Some(ext) = &unsupported {
            ui.colored_label(
                pal.bad,
                format!(
                    ".{ext} cannot carry a cover picture. Use MP4, M4V or MKV — \\
                     convert the video first if it is something else."
                ),
            );
        }

        let output = self.video.as_deref().map(output_path);
        if let Some(out) = &output {
            ui.label(egui::RichText::new(format!("Saves a new file: {}", out.display())).weak());
            ui.label(egui::RichText::new("Your original video is never touched.").weak());
        }
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            let ready = self.video.is_some()
                && self.image.is_some()
                && unsupported.is_none()
                && state != State::Working;
            let label = egui::RichText::new("Add thumbnail to video")
                .color(pal.on_accent)
                .strong()
                .size(15.0);
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(label)
                        .fill(pal.accent)
                        .corner_radius(6)
                        .min_size(egui::vec2(220.0, 42.0)),
                )
                .clicked()
            {
                if let (Some(v), Some(i)) = (self.video.clone(), self.image.clone()) {
                    self.run(v, i, ctx.clone());
                }
            }
            match &state {
                State::Working => {
                    ui.add(egui::Spinner::new());
                    ui.label("Working…");
                }
                State::Done(p) => {
                    ui.colored_label(pal.ok, format!("Done — saved {}", p.display()));
                    if ui.button("Show folder").clicked() {
                        if let Some(dir) = p.parent() {
                            let _ = Command::new("xdg-open").arg(dir).spawn();
                        }
                    }
                }
                State::Failed(m) => {
                    ui.colored_label(pal.bad, format!("Failed: {m}"));
                }
                State::Idle => {}
            }
        });
    }

    fn run(&mut self, video: PathBuf, image: PathBuf, ctx: egui::Context) {
        let state = self.state.clone();
        *state.lock().unwrap() = State::Working;
        thread::spawn(move || {
            let result = embed(&video, &image);
            *state.lock().unwrap() = match result {
                Ok(p) => State::Done(p),
                Err(e) => State::Failed(e),
            };
            ctx.request_repaint();
        });
    }
}

/// One of the two cards. Returns true when its button was pressed.
fn slot(
    ui: &mut egui::Ui,
    pal: &Palette,
    title: &str,
    preview: &mut Preview,
    file: Option<&Path>,
    button: &str,
    empty: &str,
) -> bool {
    let mut pressed = false;
    egui::Frame::new()
        .inner_margin(egui::Margin::same(12))
        .corner_radius(8)
        .fill(pal.row)
        .stroke(egui::Stroke::new(1.0_f32, pal.row_stroke))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).strong());
            ui.add_space(6.0);

            let width = ui.available_width().min(PREVIEW_W as f32);
            let size = egui::vec2(width, width * PREVIEW_H as f32 / PREVIEW_W as f32);
            if let Some(px) = preview.pixels.clone() {
                let tex = preview.tex.get_or_insert_with(|| {
                    ui.ctx().load_texture(
                        format!("cover-{title}"),
                        egui::ColorImage::from_rgba_unmultiplied([PREVIEW_W, PREVIEW_H], &px),
                        egui::TextureOptions::LINEAR,
                    )
                });
                ui.add(egui::Image::new(&*tex).fit_to_exact_size(size).corner_radius(4));
            } else {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_stroke(
                    rect,
                    4,
                    egui::Stroke::new(1.0_f32, pal.row_stroke),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if file.is_some() { "no preview" } else { empty },
                    egui::FontId::proportional(13.0),
                    ui.visuals().weak_text_color(),
                );
            }

            ui.add_space(6.0);
            if let Some(f) = file {
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                ui.add(egui::Label::new(egui::RichText::new(&name).weak()).truncate())
                    .on_hover_text(f.display().to_string());
            }
            if ui.button(button).clicked() {
                pressed = true;
            }
        });
    pressed
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn is_image(p: &Path) -> bool {
    IMAGE_EXTS.contains(&ext_of(p).as_str())
}

/// MP4 and M4V hold the picture as an attached cover; Matroska as an
/// attachment. MOV, AVI and the rest have nowhere to put one.
fn supports_cover(p: &Path) -> bool {
    matches!(ext_of(p).as_str(), "mp4" | "m4v" | "mkv")
}

/// `clip.mp4` → `clip-cover.mp4` next to it, then `clip-cover(1).mp4` … so an
/// earlier result is never overwritten.
fn output_path(video: &Path) -> PathBuf {
    let dir = video.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = video
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".into());
    let ext = ext_of(video);
    let mut out = dir.join(format!("{stem}-cover.{ext}"));
    let mut n = 1;
    while out.exists() {
        out = dir.join(format!("{stem}-cover({n}).{ext}"));
        n += 1;
    }
    out
}

/// Write `video` + `image` to a new file. Whatever the picture's format, it
/// goes through a JPEG first — the one format every container and player takes.
fn embed(video: &Path, image: &Path) -> Result<PathBuf, String> {
    let out = output_path(video);
    let jpeg = std::env::temp_dir().join(format!("vidnux-cover-{}.jpg", std::process::id()));

    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(image)
        .args(["-frames:v", "1", "-q:v", "2"])
        .arg(&jpeg)
        .output()
        .map_err(|e| format!("ffmpeg could not be started: {e}"))?;
    if !made.status.success() {
        return Err(format!(
            "could not read the picture: {}",
            String::from_utf8_lossy(&made.stderr).trim()
        ));
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-y", "-i"]).arg(video);
    if ext_of(video) == "mkv" {
        cmd.args(["-map", "0", "-map_metadata", "0", "-c", "copy", "-attach"])
            .arg(&jpeg)
            .args([
                "-metadata:s:t",
                "mimetype=image/jpeg",
                "-metadata:s:t",
                "filename=cover.jpg",
            ]);
    } else {
        // `0:V` is every video stream except an existing cover, so a second
        // run replaces the picture instead of stacking another one.
        cmd.arg("-i")
            .arg(&jpeg)
            .args([
                "-map", "1:0", "-map", "0:V", "-map", "0:a?", "-map", "0:s?",
                "-map_metadata", "0", "-c", "copy", "-disposition:v:0", "attached_pic",
            ]);
    }
    let res = cmd.arg(&out).output();
    let _ = std::fs::remove_file(&jpeg);
    let res = res.map_err(|e| format!("ffmpeg could not be started: {e}"))?;
    if res.status.success() {
        Ok(out)
    } else {
        let _ = std::fs::remove_file(&out);
        Err(String::from_utf8_lossy(&res.stderr).trim().to_string())
    }
}
