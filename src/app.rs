//! The egui front end: settings on the left, the queue in the middle.

use crate::boot;
use crate::media;
use crate::job::{self, Job, Runner, Status};
use crate::preset::{self, Profile, Settings, Target, AUDIOS, PROFILES, QUALITIES, TARGETS};
use crate::splash;
use crate::theme::{self, Palette};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Home,
    Convert,
    Cover,
}

pub struct App {
    screen: Screen,
    cover: crate::cover::Cover,
    rename_help: bool,
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
            screen: Screen::Home,
            cover: crate::cover::Cover::new(),
            rename_help: false,
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
            job.stem = render_stem(&self.rename_pattern, &name, &res, &fps, i, total);
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

    fn home(&mut self, ctx: &egui::Context, pal: &Palette) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.18).max(20.0));
                ui.label(egui::RichText::new(splash::NAME).size(30.0).strong());
                ui.label(egui::RichText::new("What would you like to do?").weak().size(15.0));
                ui.add_space(24.0);
            });
            let gap = 18.0;
            let w = ((ui.available_width() - gap) / 2.0).min(380.0);
            let total = w * 2.0 + gap;
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                if home_card(
                    ui,
                    pal,
                    w,
                    "Convert videos",
                    "Turn footage into editor-ready files (DNxHR, ProRes, H.264 …) with the quality and frame rate kept.",
                ) {
                    self.screen = Screen::Convert;
                }
                ui.add_space(gap - ui.spacing().item_spacing.x);
                if home_card(
                    ui,
                    pal,
                    w,
                    "Cover art",
                    "Choose a picture and set it as a video's thumbnail. The video is copied, not re-encoded.",
                ) {
                    self.screen = Screen::Cover;
                }
            });
        });
    }

    fn rename_help_modal(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.rename_help {
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("rename-help")).show(ctx, |ui| {
            ui.set_width(480.0_f32.min(ctx.screen_rect().width() - 80.0));
            ui.heading("Rename all — how to use it");
            ui.add_space(6.0);
            ui.label(
                "Type a pattern, press Apply to all, and every file in the queue gets a new \
                 output name. You can still edit any single name afterwards.",
            );

            ui.add_space(10.0);
            ui.label(egui::RichText::new("Placeholders").strong());
            egui::Grid::new("rename-tokens")
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for (token, what) in [
                        ("{name}", "the original file name"),
                        ("{n}", "position in the queue: 1, 2, 3 …"),
                        ("{n+1}", "position, starting at 2 (any number works: {n+3} starts at 4)"),
                        ("{n-1}", "position, starting at 0"),
                        ("{res}", "video height, like 1080p"),
                        ("{fps}", "frame rate, like 30fps"),
                    ] {
                        ui.label(egui::RichText::new(token).monospace().color(pal.accent));
                        ui.label(what);
                        ui.end_row();
                    }
                });

            ui.add_space(10.0);
            ui.label(egui::RichText::new("Examples").strong());
            ui.label(egui::RichText::new("for files  clipA, clipB, clipC").weak());
            egui::Grid::new("rename-examples")
                .num_columns(3)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for pattern in ["{name}_proxy", "scene_{n}", "scene_{n+1}", "shot{n+9}_{res}"] {
                        let result = ["clipA", "clipB", "clipC"]
                            .iter()
                            .enumerate()
                            .map(|(i, n)| render_stem(pattern, n, "1080p", "30fps", i, 3))
                            .collect::<Vec<_>>()
                            .join(", ");
                        ui.label(egui::RichText::new(pattern).monospace().color(pal.accent));
                        ui.label("→");
                        ui.label(egui::RichText::new(result).monospace());
                        ui.end_row();
                    }
                });

            ui.add_space(10.0);
            ui.label(egui::RichText::new("Good to know").strong());
            ui.label("•  Numbers are padded so files sort correctly (01 … 10).");
            ui.label("•  If two files end up with the same name, -1, -2 … is added.");
            ui.label("•  Sort A-Z / Z-A first if you want the numbers in name order.");
            ui.label("•  The extension is added for you; do not type it.");

            ui.add_space(12.0);
            if ui.button("  Got it  ").clicked() {
                ui.close();
            }
        });
        if modal.should_close() {
            self.rename_help = false;
        }
    }

    /// Predicted bytes for the jobs still to run, the free space where they
    /// will be written, and how many jobs could not be estimated. `None` when
    /// nothing is waiting.
    fn space_forecast(&self) -> Option<(u64, Option<u64>, usize)> {
        let settings = self.settings();
        let q = self.runner.queue.lock().unwrap();
        let (mut need, mut unknown, mut any) = (0u64, 0usize, false);
        let mut dir = self.out_dir.clone();
        for job in q.jobs.iter().filter(|j| !j.status.is_finished()) {
            any = true;
            match &job.info {
                Some(i) => need += preset::estimate_bytes(i, &settings),
                None => unknown += 1,
            }
            if dir.is_none() {
                dir = job.input.parent().map(|p| p.to_path_buf());
            }
        }
        any.then(|| (need, dir.and_then(|d| free_space(&d)), unknown))
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
            match self.screen {
                Screen::Cover => self.cover.accept(dropped, ctx),
                // Dropping videos on the home screen is a clear enough wish.
                _ => {
                    self.screen = Screen::Convert;
                    self.add_paths(dropped, ctx);
                }
            }
        }

        // Panels must be declared outer-first: the bottom bar has to claim its
        // strip *before* the central panel measures itself, otherwise the
        // central panel sizes itself to the whole window and the bottom bar is
        // painted over its last row — which is exactly why the final queue
        // entry used to be unreachable however far you scrolled.
        self.top_bar(ctx, &pal);
        match self.screen {
            Screen::Home => self.home(ctx, &pal),
            Screen::Convert => {
                self.side_panel(ctx, &pal);
                self.bottom_bar(ctx, &pal);
                self.queue_panel(ctx, &pal);
            }
            Screen::Cover => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                        ui.add_space(4.0);
                        self.cover.ui(ui, &pal);
                    });
                });
            }
        }
        self.rename_help_modal(ctx, &pal);

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
                    if self.screen != Screen::Home && ui.button("<  Home").clicked() {
                        self.screen = Screen::Home;
                    }
                    if self.screen == Screen::Convert {
                        ui.separator();
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
                                "About {} {} — what it does to your footage, and why.\nOpens the handbook in your browser.",
                                splash::NAME,
                                splash::VERSION,
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
                                let thumb_w = if q.jobs[i].thumb.is_some() { 96.0 + ui.spacing().item_spacing.x } else { 0.0 };
                                let left_w = (avail - status_w - thumb_w - ui.spacing().item_spacing.x).max(140.0);

                                ui.horizontal_top(|ui| {
                                    if let Some(px) = q.jobs[i].thumb.clone() {
                                        let tex = q.jobs[i].thumb_tex.get_or_insert_with(|| {
                                            ui.ctx().load_texture(
                                                format!("thumb-{id}"),
                                                egui::ColorImage::from_rgba_unmultiplied(
                                                    [media::THUMB_W, media::THUMB_H],
                                                    &px,
                                                ),
                                                egui::TextureOptions::LINEAR,
                                            )
                                        });
                                        ui.add(
                                            egui::Image::new(&*tex)
                                                .fit_to_exact_size(egui::vec2(96.0, 54.0))
                                                .corner_radius(4),
                                        );
                                    }
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
                ui.horizontal(|ui| {
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
                    if let Some((need, free, unknown)) = self.space_forecast() {
                        let tight = free.is_some_and(|f| need > f);
                        let mut text = format!("· needs ~{}", human_bytes(need));
                        if let Some(f) = free {
                            text += &format!(" of {} free", human_bytes(f));
                        }
                        let color = if tight { pal.bad } else { pal.warn };
                        ui.label(egui::RichText::new(text).color(color).strong())
                            .on_hover_text(format!(
                                "Estimated size of everything still waiting, from the current \
                                 format and quality. Actual size depends on the footage, so \
                                 keep some headroom.{}",
                                if unknown > 0 {
                                    format!(" {unknown} unreadable file(s) are not counted.")
                                } else {
                                    String::new()
                                }
                            ));
                    }
                });

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
/// The handbook file on disk if there is one — the copy `install.sh` put in
/// `share/vidnux/docs`, or the `docs/` folder of the checkout this was built
/// from — otherwise the hosted copy.
fn handbook_target() -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")));
    let candidates = [
        data.map(|d| d.join("vidnux/docs/index.html")),
        Some(PathBuf::from("/usr/local/share/vidnux/docs/index.html")),
        Some(PathBuf::from("/usr/share/vidnux/docs/index.html")),
        Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/index.html"))),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| HANDBOOK.to_string())
}

fn open_handbook() {
    let target = handbook_target();
    for opener in ["xdg-open", "gio", "x-www-browser", "firefox"] {
        let mut cmd = std::process::Command::new(opener);
        if opener == "gio" {
            cmd.arg("open");
        }
        let spawned = cmd
            .arg(&target)
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
        );

        let row = |ui: &mut egui::Ui, me: &mut Self| {
            if ui
                .small_button("?")
                .on_hover_text("How to use Rename all")
                .clicked()
            {
                me.rename_help = true;
                ui.close();
            }
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

/// Fill in every placeholder of the rename pattern for one row.
fn render_stem(pattern: &str, name: &str, res: &str, fps: &str, index: usize, total: usize) -> String {
    expand_counter(&pattern.replace("{name}", name), index, total)
        .replace("{res}", res)
        .replace("{fps}", fps)
}

/// A big clickable card for the home screen. True when clicked.
fn home_card(ui: &mut egui::Ui, pal: &Palette, width: f32, title: &str, text: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 170.0), egui::Sense::click());
    let hot = resp.hovered();
    ui.painter().rect(
        rect,
        10,
        if hot { pal.row_hover } else { pal.row },
        egui::Stroke::new(if hot { 1.5_f32 } else { 1.0_f32 }, if hot { pal.accent } else { pal.row_stroke }),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(18.0);
    let title_galley = ui.painter().layout_no_wrap(
        title.to_string(),
        egui::FontId::proportional(20.0),
        pal.accent,
    );
    let body = ui.painter().layout(
        text.to_string(),
        egui::FontId::proportional(13.5),
        ui.visuals().text_color(),
        inner.width(),
    );
    let gap = 10.0;
    ui.painter().galley(inner.min, title_galley.clone(), pal.accent);
    ui.painter().galley(
        inner.min + egui::vec2(0.0, title_galley.size().y + gap),
        body,
        ui.visuals().text_color(),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Free bytes on the filesystem holding `dir`.
fn free_space(dir: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0)
        .then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i < 3 { format!("{v:.0} {}", U[i]) } else { format!("{v:.1} {}", U[i]) }
}

/// Replace `{n}`, `{n+K}` and `{n-K}` with the row's position. `{n}` counts
/// from 1; `{n+3}` counts from 4, `{n-1}` from 0. Numbers are zero-padded to
/// the width of the largest one, so the files sort the way they read. A shift
/// that would go below zero stops at 0.
fn expand_counter(pattern: &str, index: usize, total: usize) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(start) = rest.find("{n") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let spec = &after[..end];
        let shift: Option<i64> = match spec {
            "" => Some(0),
            _ => {
                let (sign, digits) = spec.split_at(1);
                match (sign, digits.parse::<i64>()) {
                    ("+", Ok(k)) => Some(k),
                    ("-", Ok(k)) => Some(-k),
                    _ => None,
                }
            }
        };
        out.push_str(&rest[..start]);
        match shift {
            Some(k) => {
                let n = |i: usize| (i as i64 + 1 + k).max(0);
                let width = n(total.saturating_sub(1)).to_string().len();
                out.push_str(&format!("{:0width$}", n(index), width = width));
            }
            // Not a counter (`{name}` is already gone, but `{nope}` is not
            // ours): leave it as typed.
            None => out.push_str(&rest[start..start + 2 + end + 1]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
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
    use super::{expand_counter, mix, natural_cmp, short};
    use std::cmp::Ordering;

    #[test]
    fn counter_can_start_later() {
        assert_eq!(expand_counter("clip_{n}", 0, 3), "clip_1");
        assert_eq!(expand_counter("clip_{n+1}", 0, 3), "clip_2");
        assert_eq!(expand_counter("clip_{n+3}", 2, 3), "clip_6");
        assert_eq!(expand_counter("{n-1}", 0, 3), "0");
        assert_eq!(expand_counter("{n+1}", 0, 9), "02");
        assert_eq!(expand_counter("{nope}-{n}", 0, 3), "{nope}-1");
    }

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
