//! The egui front end: settings on the left, the queue in the middle.

use crate::boot;
use crate::job::{self, Job, Runner, Status};
use crate::preset::{self, Profile, Settings, Target, AUDIOS, PROFILES, QUALITIES, TARGETS};
use crate::splash;
use crate::theme::{self, Palette};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct App {
    runner: Runner,
    out_dir: Option<PathBuf>,
    rename_pattern: String,
    adding: Arc<Mutex<usize>>,
    show_command: bool,
    sort_desc: bool,
    theme_mode: theme::Mode,
    theme_watch: theme::Watcher,
    /// Start-up checks; the banner is shown until these finish.
    boot: boot::Shared,
    /// Copied out of `boot` once it is done, so the settings panel does not
    /// take a lock on every frame.
    ready: bool,
    encoders: HashSet<&'static str>,
    /// Row under the pointer, for the queue's hover highlight.
    hovered: Option<u64>,
    /// Row to keep on screen after it was nudged past the viewport edge.
    scroll_to: Option<u64>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        style(&cc.egui_ctx);
        // Lets the banner load `assets/splash.svg`.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        Self {
            runner: Runner::new(),
            out_dir: None,
            rename_pattern: "{name}".to_string(),
            adding: Arc::new(Mutex::new(0)),
            show_command: false,
            sort_desc: false,
            theme_mode: theme::Mode::default(),
            theme_watch: theme::Watcher::spawn(&cc.egui_ctx),
            boot: boot::start(&cc.egui_ctx),
            ready: false,
            encoders: HashSet::new(),
            hovered: None,
            scroll_to: None,
        }
    }

    fn settings(&self) -> Settings {
        self.runner.settings.lock().unwrap().clone()
    }

    /// Can the local ffmpeg actually produce this target? Everything is
    /// allowed until the start-up check has answered.
    fn supports(&self, target: Target) -> bool {
        self.encoders.is_empty() || self.encoders.contains(target.encoder())
    }

    /// Resolve `Auto` against what the desktop reports and hand egui the
    /// matching theme. Our own detection wins over the windowing backend,
    /// which on Linux usually reports nothing at all; when neither knows, the
    /// app stays light.
    fn apply_theme(&self, ctx: &egui::Context) -> Palette {
        let wanted = match self.theme_mode {
            theme::Mode::Auto => self
                .theme_watch
                .get()
                .or_else(|| ctx.system_theme())
                .unwrap_or(egui::Theme::Light),
            theme::Mode::Light => egui::Theme::Light,
            theme::Mode::Dark => egui::Theme::Dark,
        };
        if ctx.theme() != wanted {
            ctx.set_theme(wanted);
        }
        theme::palette(wanted)
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    let mut found: Vec<PathBuf> = rd
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.is_file() && job::is_video(p))
                        .collect();
                    found.sort();
                    files.extend(found);
                }
            } else if p.is_file() {
                files.push(p);
            }
        }
        if files.is_empty() {
            return;
        }
        *self.adding.lock().unwrap() += files.len();
        let queue = self.runner.queue.clone();
        let adding = self.adding.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            for f in files {
                let already = queue
                    .lock()
                    .unwrap()
                    .jobs
                    .iter()
                    .any(|j| j.input == f && !j.status.is_finished());
                if !already {
                    let job = Job::new(f);
                    queue.lock().unwrap().jobs.push(job);
                }
                *adding.lock().unwrap() -= 1;
                ctx.request_repaint();
            }
        });
    }

    fn browse_files(&mut self, ctx: &egui::Context) {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("Add videos")
            .add_filter("Video files", job::VIDEO_EXTS)
            .add_filter("All files", &["*"])
            .pick_files()
        {
            self.add_paths(files, ctx);
        }
    }

    /// Apply the rename pattern to every row, then make the result unique:
    /// a pattern without `{n}` would give every file the same name, so
    /// `Adam-Kun` becomes `Adam-Kun-1`, `Adam-Kun-2`, … when it repeats.
    fn apply_pattern(&mut self) {
        let mut q = self.runner.queue.lock().unwrap();
        let total = q.jobs.len();
        for (i, job) in q.jobs.iter_mut().enumerate() {
            let name = job
                .input
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let (res, fps) = job
                .info
                .as_ref()
                .map(|i| (format!("{}p", i.height), format!("{:.0}fps", i.fps)))
                .unwrap_or_default();
            let width = total.to_string().len();
            job.stem = self
                .rename_pattern
                .replace("{name}", &name)
                .replace("{n}", &format!("{:0width$}", i + 1, width = width))
                .replace("{res}", &res)
                .replace("{fps}", &fps);
        }

        let mut totals: HashMap<String, usize> = HashMap::new();
        for job in q.jobs.iter() {
            *totals.entry(job.stem.clone()).or_default() += 1;
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        for job in q.jobs.iter_mut() {
            if totals.get(&job.stem).copied().unwrap_or(0) > 1 {
                let n = seen.entry(job.stem.clone()).or_default();
                *n += 1;
                job.stem = format!("{}-{}", job.stem, n);
            }
        }
    }

    /// Sort the queue by file name; pressing the button again flips A-Z / Z-A.
    fn sort_by_name(&mut self) {
        self.sort_desc = !self.sort_desc;
        let desc = self.sort_desc;
        let mut q = self.runner.queue.lock().unwrap();
        q.jobs.sort_by(|a, b| {
            let (a, b) = (a.name().to_lowercase(), b.name().to_lowercase());
            if desc {
                natural_cmp(&b, &a)
            } else {
                natural_cmp(&a, &b)
            }
        });
    }

    /// Turn the banner window into the interface: give it back its title bar,
    /// free it from the banner's fixed size, and grow it to the working size.
    /// Done once, on the frame the checks finish.
    ///
    /// The window has been `resizable(true)` since it was created — see the
    /// comment in `main.rs` on why toggling that flag at runtime is not
    /// trustworthy on Wayland. What actually held the banner still was its
    /// min and max size hints pinned equal to its own size, so freeing it here
    /// means relaxing those hints, not flipping `resizable` on.
    fn become_main_window(&self, ctx: &egui::Context) {
        use egui::ViewportCommand as Cmd;
        for cmd in [
            Cmd::MinInnerSize(crate::MIN_WINDOW.into()),
            // Infinity is egui-winit's sentinel for "no cap at all" — it
            // clears the OS-level hint outright rather than substituting some
            // arbitrarily large number that would still have to be picked.
            Cmd::MaxInnerSize(egui::Vec2::INFINITY),
            Cmd::InnerSize(crate::WINDOW.into()),
            Cmd::Decorations(true),
        ] {
            ctx.send_viewport_cmd(cmd);
        }
    }

    /// Move the row at `from` to `to` — always a neighbour.
    fn swap_rows(&mut self, from: usize, to: usize) {
        let mut q = self.runner.queue.lock().unwrap();
        if from >= q.jobs.len() || to >= q.jobs.len() {
            return;
        }
        q.jobs.swap(from, to);
        let id = q.jobs[to].id;
        drop(q);
        // Keep the row the user is moving in view, so holding the button walks
        // it up or down the list without losing sight of it.
        self.scroll_to = Some(id);
    }

    /// Queue totals for the status strip.
    fn tally(&self) -> Tally {
        let q = self.runner.queue.lock().unwrap();
        let mut t = Tally {
            total: q.jobs.len(),
            ..Default::default()
        };
        for job in q.jobs.iter() {
            match &job.status {
                Status::Queued => {}
                Status::Running => t.running += 1,
                Status::Done => t.done += 1,
                Status::Canceled | Status::Failed(_) => t.settled += 1,
            }
        }
        t.queued = t.total - t.running - t.done - t.settled;
        t
    }
}

#[derive(Default)]
struct Tally {
    total: usize,
    done: usize,
    queued: usize,
    running: usize,
    /// Cancelled or failed — finished, but not successfully.
    settled: usize,
}

fn style(ctx: &egui::Context) {
    ctx.options_mut(|o| {
        o.theme_preference = egui::ThemePreference::System;
        // What egui falls back to before our own detection has answered.
        o.fallback_theme = egui::Theme::Light;
    });
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.interact_size.y = 26.0;
        // A scrollbar in its own lane rather than a bar floating over the
        // rows, so nothing is ever hidden underneath it.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 9.0;
        style.spacing.scroll.bar_inner_margin = 6.0;
        style.visuals.window_corner_radius = 10.into();

        let pal = theme::palette(if style.visuals.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        // On a dark ground a translucent accent reads fine; on a light one it
        // would muddy the text underneath, so tint towards white instead.
        style.visuals.selection.bg_fill = if style.visuals.dark_mode {
            pal.accent.gamma_multiply(0.45)
        } else {
            mix(egui::Color32::WHITE, pal.accent, 0.35)
        };
        style.visuals.selection.stroke.color = pal.accent;
        style.visuals.hyperlink_color = pal.accent;

        for (_, id) in style.text_styles.iter_mut() {
            id.size *= 1.05;
        }
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pal = self.apply_theme(ctx);

        if !self.ready {
            let p = self.boot.lock().unwrap().clone();
            if p.ready {
                self.ready = true;
                self.encoders = p.encoders;
                self.become_main_window(ctx);
            } else {
                splash::show(ctx, &p, &pal);
                return;
            }
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped, ctx);
        }

        // Panels must be declared outer-first: the bottom bar has to claim its
        // strip *before* the central panel measures itself, otherwise the
        // central panel sizes itself to the whole window and the bottom bar is
        // painted over its last row — which is exactly why the final queue
        // entry used to be unreachable however far you scrolled.
        self.top_bar(ctx, &pal);
        self.side_panel(ctx, &pal);
        self.bottom_bar(ctx, &pal);
        self.queue_panel(ctx, &pal);

        if self.runner.running.load(Ordering::SeqCst) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.theme_watch.stop();
    }
}

impl App {
    fn top_bar(&mut self, ctx: &egui::Context, _pal: &Palette) {
        egui::TopBottomPanel::top("top")
            .exact_height(46.0)
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    if ui.button("+  Add files").clicked() {
                        let ctx = ui.ctx().clone();
                        self.browse_files(&ctx);
                    }
                    if ui.button("Add folder").clicked() {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Add every video in a folder")
                            .pick_folder()
                        {
                            let ctx = ui.ctx().clone();
                            self.add_paths(vec![dir], &ctx);
                        }
                    }
                    ui.separator();
                    if ui.button("Clear finished").clicked() {
                        self.runner
                            .queue
                            .lock()
                            .unwrap()
                            .jobs
                            .retain(|j| !j.status.is_finished());
                    }
                    if ui.button("Clear all").clicked() {
                        self.runner.stop();
                        self.runner.queue.lock().unwrap().jobs.clear();
                    }
                    let pending = *self.adding.lock().unwrap();
                    if pending > 0 {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(egui::RichText::new(format!("reading {pending} file(s)…")).weak());
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(4.0);
                        let detected = match self.theme_watch.get() {
                            Some(egui::Theme::Dark) => "desktop is set to dark",
                            Some(egui::Theme::Light) => "desktop is set to light",
                            None => "could not read the desktop preference — using light",
                        };
                        if ui
                            .button("  ?  ")
                            .on_hover_text(format!(
                                "About {} {} — what it does to your footage, and why.\nOpens {}",
                                splash::NAME,
                                splash::VERSION,
                                HANDBOOK
                            ))
                            .clicked()
                        {
                            open_handbook();
                        }
                        if ui
                            .button(self.theme_mode.label())
                            .on_hover_text(format!(
                                "Follow the desktop, or force light / dark.\n{detected}"
                            ))
                            .clicked()
                        {
                            self.theme_mode = self.theme_mode.next();
                        }
                    });
                });
            });
    }

    fn side_panel(&mut self, ctx: &egui::Context, pal: &Palette) {
        // No separator line and a fill of its own: the division between the
        // settings and the queue reads as a change of surface instead of the
        // dark rule egui draws by default.
        let frame = egui::Frame::new()
            .fill(pal.surface)
            .inner_margin(egui::Margin::symmetric(12, 0));

        egui::SidePanel::left("settings")
            .exact_width(316.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(frame)
            .show(ctx, |ui| {
                // Scrolls, so every control stays reachable however short the
                // window gets.
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| self.settings_controls(ui, pal));
            });
    }

    fn settings_controls(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.add_space(12.0);
        let mut s = self.settings();
        let before = s.clone();
        // The scroll area has already given back the scrollbar's lane; just
        // stay off the edge.
        let w = ui.available_width() - 2.0;

        heading(ui, "OUTPUT FORMAT");
        egui::ComboBox::from_id_salt("target")
            .width(w)
            .truncate()
            .selected_text(short(s.target.label()))
            .show_ui(ui, |ui| {
                for t in TARGETS {
                    let ok = self.supports(t);
                    ui.add_enabled_ui(ok, |ui| {
                        ui.selectable_value(&mut s.target, t, t.label())
                            .on_disabled_hover_text(format!(
                                "This ffmpeg build has no {} encoder.",
                                t.encoder()
                            ));
                    });
                }
            })
            .response
            // The long explanation used to sit under the box and pushed
            // everything else off the panel; it is one hover away instead.
            .on_hover_text(s.target.hint());
        if !self.supports(s.target) {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "Your ffmpeg has no {} encoder — this format will fail.",
                    s.target.encoder()
                ))
                .color(pal.bad)
                .size(11.5),
            );
        }
        ui.add_space(14.0);

        if s.target.uses_quality() {
            heading(ui, "QUALITY");
            egui::ComboBox::from_id_salt("quality")
                .width(w)
                .truncate()
                .selected_text(s.quality.label())
                .show_ui(ui, |ui| {
                    for q in QUALITIES {
                        ui.selectable_value(&mut s.quality, q, q.label());
                    }
                });
            ui.add_space(14.0);
        }

        if s.target.uses_profile() {
            heading(ui, "PROFILE");
            egui::ComboBox::from_id_salt("profile")
                .width(w)
                .truncate()
                .selected_text(s.profile.label(s.target))
                .show_ui(ui, |ui| {
                    for p in PROFILES {
                        ui.selectable_value(&mut s.profile, p, p.label(s.target));
                    }
                });
            ui.add_space(14.0);
        }

        if s.target.is_lossless() {
            ui.label(
                egui::RichText::new("Every pixel is preserved exactly — expect large files.")
                    .color(pal.ok)
                    .size(11.5),
            );
            ui.add_space(14.0);
        }

        heading(ui, "AUDIO");
        egui::ComboBox::from_id_salt("audio")
            .width(w)
            .truncate()
            .selected_text(s.audio.label())
            .show_ui(ui, |ui| {
                for a in AUDIOS {
                    ui.selectable_value(&mut s.audio, a, a.label());
                }
            });
        ui.add_space(16.0);

        heading(ui, "DESTINATION");
        ui.horizontal(|ui| {
            if ui.button("Choose…").clicked() {
                if let Some(d) = rfd::FileDialog::new()
                    .set_title("Where should the converted files go?")
                    .pick_folder()
                {
                    self.out_dir = Some(d);
                }
            }
            if self.out_dir.is_some() && ui.button("Reset").clicked() {
                self.out_dir = None;
            }
        });
        let dest = match &self.out_dir {
            Some(d) => d.display().to_string(),
            None => "next to each source file".into(),
        };
        ui.add(egui::Label::new(egui::RichText::new(dest).weak().size(11.5)).truncate());
        ui.add_space(16.0);

        ui.checkbox(&mut s.keep_all_tracks, "Keep every audio / subtitle track");
        ui.checkbox(&mut s.overwrite, "Overwrite existing files");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Run at once");
            let mut c = s.concurrency as u32;
            if ui.add(egui::DragValue::new(&mut c).range(1..=8)).changed() {
                s.concurrency = c as usize;
            }
        });
        ui.add_space(10.0);

        ui.collapsing("Advanced", |ui| {
            ui.label(
                egui::RichText::new("Extra ffmpeg arguments")
                    .weak()
                    .size(11.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut s.extra_args)
                    .hint_text("-metadata title=…")
                    .desired_width(ui.available_width() - 2.0),
            );
            ui.checkbox(&mut self.show_command, "Show the ffmpeg command per file");
        });

        if s.target != before.target {
            // Profiles are shared between DNxHR and ProRes; keep a sane one.
            if s.profile == Profile::Fourfourfour && s.target == Target::DnxhrMov {
                s.profile = Profile::Hq;
            }
        }
        if s != before {
            *self.runner.settings.lock().unwrap() = s;
        }

        ui.add_space(18.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(splash::NAME)
                    .strong()
                    .size(12.0)
                    .color(pal.accent),
            );
            ui.label(egui::RichText::new(splash::VERSION).weak().size(12.0));
        });
        ui.label(
            egui::RichText::new(format!("by {}", splash::AUTHOR))
                .weak()
                .size(11.0),
        );
        ui.add_space(12.0);
    }

    fn queue_panel(&mut self, ctx: &egui::Context, pal: &Palette) {
        let settings = self.settings();
        let out_dir = self.out_dir.clone();
        let show_command = self.show_command;
        let was_hovered = self.hovered;
        let scroll_to = self.scroll_to.take();

        let mut remove: Option<u64> = None;
        let mut cancel: Option<u64> = None;
        let mut swap: Option<(usize, usize)> = None;
        let mut hovered: Option<u64> = None;

        let queue = self.runner.queue.clone();

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut q = queue.lock().unwrap();
            if q.jobs.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "Drop video files here\n\nor use \"Add files\" - you can select as many as you like",
                        )
                        .size(15.0)
                        .weak(),
                    );
                });
                return;
            }

            egui::ScrollArea::vertical()
                // Always fill the panel, so the list keeps its own height
                // instead of collapsing around the rows.
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let len = q.jobs.len();
                    for i in 0..len {
                        let (id, header, badge, badge_color, status, progress, speed, source, out_path) = {
                            let job = &q.jobs[i];
                            let badge = match &job.info {
                                Some(info) => format!("{}  ·  {}", info.class(), info.summary()),
                                None => job
                                    .probe_error
                                    .clone()
                                    .unwrap_or_else(|| "reading…".into()),
                            };
                            let color = if job.info.is_some() { pal.warn } else { pal.bad };
                            (
                                job.id,
                                job.name(),
                                badge,
                                color,
                                job.status.clone(),
                                job.progress,
                                job.speed.clone(),
                                job.input.clone(),
                                job.target_path(&out_dir, &settings),
                            )
                        };

                        let hot = was_hovered == Some(id);
                        let row = egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(12, 9))
                            .corner_radius(8)
                            .fill(if hot { pal.row_hover } else { pal.row })
                            .stroke(egui::Stroke::new(1.0_f32, pal.row_stroke))
                            .show(ui, |ui| {
                                // Split the row proportionally so it keeps
                                // working from a narrow window up to a wide one.
                                let avail = ui.available_width();
                                let status_w = (avail * 0.26).clamp(130.0, 200.0);
                                let left_w =
                                    (avail - status_w - ui.spacing().item_spacing.x).max(140.0);

                                ui.horizontal_top(|ui| {
                                    ui.vertical(|ui| {
                                        ui.set_width(left_w);
                                        ui.horizontal(|ui| {
                                            // Long file names get an ellipsis
                                            // instead of spilling past the panel.
                                            ui.scope(|ui| {
                                                ui.set_max_width((left_w * 0.58).max(110.0));
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(&header).strong(),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(&header);
                                            });
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&badge)
                                                        .color(badge_color)
                                                        .size(11.5),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&badge);
                                        });
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("->").weak());
                                            let ext = format!(".{}", settings.target.ext());
                                            let name_w = (left_w
                                                - 34.0
                                                - 10.0 * ext.len() as f32)
                                                .max(80.0);
                                            let job = &mut q.jobs[i];
                                            ui.add(
                                                egui::TextEdit::singleline(&mut job.stem)
                                                    .desired_width(name_w)
                                                    .hint_text("output name"),
                                            );
                                            ui.label(
                                                egui::RichText::new(ext).color(pal.accent),
                                            );
                                        });
                                        if show_command {
                                            if let Some(info) = q.jobs[i].info.clone() {
                                                ui.add_space(4.0);
                                                let cmd = preset::preview(
                                                    &source, &out_path, &info, &settings,
                                                );
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(cmd)
                                                            .monospace()
                                                            .weak()
                                                            .size(10.5),
                                                    )
                                                    .wrap(),
                                                );
                                            }
                                        }
                                    });

                                    ui.vertical(|ui| {
                                        ui.set_width(status_w);
                                        match &status {
                                            Status::Queued => {
                                                ui.label(egui::RichText::new("Queued").weak());
                                            }
                                            Status::Running => {
                                                ui.add(
                                                    egui::ProgressBar::new(progress)
                                                        .desired_height(10.0)
                                                        .fill(pal.accent)
                                                        .show_percentage(),
                                                );
                                                if !speed.is_empty() {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{speed} realtime"
                                                        ))
                                                        .weak()
                                                        .size(11.0),
                                                    );
                                                }
                                            }
                                            Status::Done => {
                                                ui.label(
                                                    egui::RichText::new("Done")
                                                        .color(pal.ok)
                                                        .strong(),
                                                );
                                            }
                                            Status::Canceled => {
                                                ui.label(egui::RichText::new("Canceled").weak());
                                            }
                                            Status::Failed(m) => {
                                                ui.label(
                                                    egui::RichText::new("Failed")
                                                        .color(pal.bad)
                                                        .strong(),
                                                );
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(m)
                                                            .color(pal.bad)
                                                            .size(11.0),
                                                    )
                                                    .wrap(),
                                                );
                                            }
                                        }
                                        ui.add_space(4.0);
                                        // Wrapped, so the buttons stack onto a
                                        // second line in a narrow window rather
                                        // than being cut off at the edge.
                                        ui.horizontal_wrapped(|ui| {
                                            if status == Status::Running {
                                                if ui.small_button("Cancel").clicked() {
                                                    cancel = Some(id);
                                                }
                                            } else {
                                                if ui.small_button("Remove").clicked() {
                                                    remove = Some(id);
                                                }
                                                if status.is_finished()
                                                    && ui.small_button("Requeue").clicked()
                                                {
                                                    q.jobs[i].status = Status::Queued;
                                                    q.jobs[i].progress = 0.0;
                                                }
                                            }
                                            if ui
                                                .add_enabled(
                                                    i > 0,
                                                    egui::Button::new(" ^ ").small(),
                                                )
                                                .on_hover_text("Move up")
                                                .clicked()
                                            {
                                                swap = Some((i, i - 1));
                                            }
                                            if ui
                                                .add_enabled(
                                                    i + 1 < len,
                                                    egui::Button::new(" v ").small(),
                                                )
                                                .on_hover_text("Move down")
                                                .clicked()
                                            {
                                                swap = Some((i, i + 1));
                                            }
                                        });
                                    });
                                });
                            });

                        if row.response.contains_pointer() {
                            hovered = Some(id);
                        }
                        if scroll_to == Some(id) {
                            row.response.scroll_to_me(None);
                        }
                    }
                });
        });

        self.hovered = hovered;

        let mut q = self.runner.queue.lock().unwrap();
        if let Some(id) = remove {
            q.jobs.retain(|j| j.id != id);
        }
        if let Some(id) = cancel {
            if let Some(job) = q.find(id) {
                job.cancel = true;
                if let Some(pid) = job.pid {
                    job::kill(pid);
                }
            }
        }
        drop(q);
        if let Some((from, to)) = swap {
            self.swap_rows(from, to);
        }
    }

    /// The status strip: what the queue holds on the first line, the renaming
    /// tools and the one big action button on the second.
    fn bottom_bar(&mut self, ctx: &egui::Context, pal: &Palette) {
        let t = self.tally();
        let is_running = self.runner.running.load(Ordering::SeqCst);

        egui::TopBottomPanel::bottom("bottom")
            .exact_height(88.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.surface)
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(ctx, |ui| {
                // The tally only. Every running row already draws its own
                // percentage, so a second bar summarising them said nothing the
                // queue was not already saying.
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} in queue · {} running · {} waiting · {} done",
                            t.total, t.running, t.queued, t.done
                        ))
                        .weak(),
                    )
                    .truncate(),
                );

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    // The action button gets its width first; the queue tools
                    // take what is left, and fold into a menu when that is not
                    // enough to lay them out side by side.
                    let total = ui.available_width();
                    let button_w = 170.0_f32.min((total * 0.45).max(96.0));
                    let tools_w = (total - button_w - ui.spacing().item_spacing.x).max(0.0);

                    ui.scope(|ui| {
                        ui.set_width(tools_w);
                        if tools_w >= TOOLS_INLINE {
                            ui.horizontal(|ui| self.queue_tools(ui, tools_w, true));
                        } else {
                            ui.menu_button("Rename / sort…", |ui| {
                                ui.set_min_width(280.0);
                                self.queue_tools(ui, 264.0, false)
                            });
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let size = egui::vec2(button_w, 42.0);
                        if is_running {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Stop")
                                            .color(pal.on_accent)
                                            .strong()
                                            .size(15.0),
                                    )
                                    .fill(pal.bad)
                                    .corner_radius(6)
                                    .min_size(size),
                                )
                                .clicked()
                            {
                                self.runner.stop();
                            }
                        } else if ui
                            .add_enabled(
                                t.queued > 0,
                                egui::Button::new(
                                    egui::RichText::new("Start converting")
                                        .color(pal.on_accent)
                                        .strong()
                                        .size(15.0),
                                )
                                .fill(pal.accent)
                                .corner_radius(6)
                                .min_size(size),
                            )
                            .clicked()
                        {
                            *self.runner.out_dir.lock().unwrap() = self.out_dir.clone();
                            self.runner.start(ctx.clone());
                        }
                    });
                });
            });
    }
}

/// Width the rename pattern, its two buttons and their labels need before they
/// are worth laying out along the bottom bar rather than behind a menu.
const TOOLS_INLINE: f32 = 460.0;

/// The handbook, served straight out of the repository by raw.githack — so the
/// page ships with the source and needs no hosting of its own. `docs/index.html`
/// is the file behind it.
const HANDBOOK: &str = "https://raw.githack.com/dewakuneiei/vidnux/main/docs/index.html";

/// Hand the URL to whatever the desktop uses for links. `xdg-open` covers every
/// desktop that follows the freedesktop spec; the others are there for the ones
/// that do not ship it.
fn open_handbook() {
    for opener in ["xdg-open", "gio", "x-www-browser", "firefox"] {
        let mut cmd = std::process::Command::new(opener);
        if opener == "gio" {
            cmd.arg("open");
        }
        let spawned = cmd
            .arg(HANDBOOK)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if spawned.is_ok() {
            return;
        }
    }
}

impl App {
    /// Rename-and-sort, shared between the bottom bar and the menu it folds
    /// into when the window is too narrow to show them side by side.
    fn queue_tools(&mut self, ui: &mut egui::Ui, width: f32, inline: bool) {
        if inline {
            ui.label(egui::RichText::new("Rename all").weak());
        }
        // The two buttons keep their labels; the pattern field absorbs the
        // rest of the width.
        let field = if inline {
            (width - 300.0).clamp(110.0, 240.0)
        } else {
            width
        };
        ui.add(
            egui::TextEdit::singleline(&mut self.rename_pattern)
                .desired_width(field)
                .hint_text("{name}_proxy"),
        )
        .on_hover_text("{name} source name · {n} position · {res} height · {fps} frame rate");

        let row = |ui: &mut egui::Ui, me: &mut Self| {
            if ui.button("Apply to all").clicked() {
                me.apply_pattern();
            }
            let sort_label = if me.sort_desc {
                "Sort: Z-A"
            } else {
                "Sort: A-Z"
            };
            if ui
                .button(sort_label)
                .on_hover_text("Sort the queue by file name; press again to reverse")
                .clicked()
            {
                me.sort_by_name();
            }
        };
        if inline {
            row(ui, self);
        } else {
            ui.horizontal(|ui| row(ui, self));
        }
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).weak().size(11.0));
    ui.add_space(3.0);
}

/// Format labels read `DNxHR / MOV  —  DaVinci Resolve ready`; the closed box
/// shows only the format, since the half after the dash is the same thing the
/// hover explanation says at length.
fn short(label: &str) -> &str {
    label.split("  —  ").next().unwrap_or(label).trim()
}

/// Blend two colours, `t` running 0 (all `a`) to 1 (all `b`).
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

/// Compare names so that `clip2` lands before `clip10`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    let mut nb = String::new();
                    while x.peek().is_some_and(|c| c.is_ascii_digit()) {
                        na.push(x.next().unwrap());
                    }
                    while y.peek().is_some_and(|c| c.is_ascii_digit()) {
                        nb.push(y.next().unwrap());
                    }
                    let va: u128 = na.parse().unwrap_or(0);
                    let vb: u128 = nb.parse().unwrap_or(0);
                    match va.cmp(&vb) {
                        std::cmp::Ordering::Equal => {}
                        other => return other,
                    }
                } else {
                    match ca.cmp(&cb) {
                        std::cmp::Ordering::Equal => {
                            x.next();
                            y.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{mix, natural_cmp, short};
    use std::cmp::Ordering;

    #[test]
    fn numbers_sort_by_value_not_by_character() {
        assert_eq!(natural_cmp("clip2.mp4", "clip10.mp4"), Ordering::Less);
        assert_eq!(natural_cmp("Adam-Kun-9", "Adam-Kun-10"), Ordering::Less);
        assert_eq!(natural_cmp("b.mp4", "a.mp4"), Ordering::Greater);
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    }

    #[test]
    fn mixing_stays_inside_the_two_ends() {
        let a = egui::Color32::from_rgb(0, 0, 0);
        let b = egui::Color32::from_rgb(200, 100, 50);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), egui::Color32::from_rgb(100, 50, 25));
        // Out-of-range factors are clamped rather than wrapping around.
        assert_eq!(mix(a, b, 2.0), b);
        assert_eq!(mix(a, b, -1.0), a);
    }

    #[test]
    fn closed_combo_boxes_drop_the_explanation_after_the_dash() {
        assert_eq!(
            short("DNxHR / MOV  —  DaVinci Resolve ready"),
            "DNxHR / MOV"
        );
        assert_eq!(short("AV1 / MKV  —  small, keeps every track"), "AV1 / MKV");
        // Labels with no dash are left exactly as they are.
        assert_eq!(short("High"), "High");
        assert_eq!(short(""), "");
    }
}
