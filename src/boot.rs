//! What the start-up banner is waiting for.
//!
//! None of this is busywork invented to justify a splash screen: the app
//! genuinely cannot lay out its settings until it knows which encoders the
//! local ffmpeg was built with. Fedora's `ffmpeg-free`, for one, ships without
//! x264 and x265, so offering those targets would only produce failed jobs.
//! The banner reports each check as it happens instead of freezing on a blank
//! window.

use crate::preset::TARGETS;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Shared between the checker thread and the banner.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// What is being done right now, shown under the wordmark.
    pub step: String,
    /// 0.0 to 1.0 across every step.
    pub fraction: f32,
    /// Lines already finished, newest last.
    pub log: Vec<String>,
    /// Set once the app can be shown.
    pub ready: bool,
    /// Set when start-up cannot continue — no ffmpeg, most likely.
    pub error: Option<String>,
    /// Targets this ffmpeg build can actually produce.
    pub encoders: HashSet<&'static str>,
    /// `ffmpeg version 7.1.1` and the like, for the About line.
    pub version: String,
}

pub type Shared = Arc<Mutex<Progress>>;

/// Every step the banner walks through, with the share of the bar it owns.
const STEPS: usize = 5;

/// The checks take about a third of a second on a warm cache, which would make
/// the banner a flicker — nobody would read the name, the version or who wrote
/// it. So the banner is held to this long in total.
///
/// It is padding only when there is time to pad: a machine that takes longer
/// than this to answer has already earned the wait, and nothing is added on top
/// of it. A cold page cache, a slow disk or an ffmpeg with a hundred encoders
/// all fall through this untouched.
const MIN_SHOWN: Duration = Duration::from_millis(2000);

/// How often the bar creeps forward while waiting out [`MIN_SHOWN`], so the
/// banner never looks stuck at the same fraction.
const TICK: Duration = Duration::from_millis(40);

/// Run the checks on their own thread so the banner keeps painting.
pub fn start(ctx: &egui::Context) -> Shared {
    let shared: Shared = Arc::new(Mutex::new(Progress {
        step: "Starting up".into(),
        ..Default::default()
    }));

    let out = shared.clone();
    let ctx = ctx.clone();
    thread::spawn(move || {
        let opened = Instant::now();
        let mut done = 0usize;
        let mut step = |name: &str, line: Option<String>| {
            let mut p = out.lock().unwrap();
            if let Some(line) = line {
                p.log.push(line);
            }
            p.step = name.to_string();
            p.fraction = done as f32 / STEPS as f32;
            done += 1;
            drop(p);
            ctx.request_repaint();
        };

        step("Looking for ffmpeg", None);
        let Some(ffmpeg) = which("ffmpeg") else {
            fail(&out, &ctx, "ffmpeg was not found on your PATH");
            return;
        };

        step(
            "Looking for ffprobe",
            Some(format!("ffmpeg    {}", ffmpeg.display())),
        );
        let Some(ffprobe) = which("ffprobe") else {
            fail(&out, &ctx, "ffprobe was not found on your PATH");
            return;
        };

        step(
            "Reading the ffmpeg build",
            Some(format!("ffprobe   {}", ffprobe.display())),
        );
        let version = version();

        step(
            "Checking which encoders are available",
            Some(format!("version   {version}")),
        );
        let encoders = encoders();

        let missing: Vec<&str> = TARGETS
            .iter()
            .filter(|t| !encoders.contains(t.encoder()))
            .map(|t| t.encoder())
            .collect();
        let summary = if missing.is_empty() {
            format!("encoders  all {} output formats available", TARGETS.len())
        } else {
            format!("encoders  missing {}", missing.join(", "))
        };

        step("Preparing the interface", Some(summary));

        // Walk the bar to full over whatever is left of the minimum, rather
        // than freezing it and then jumping.
        let from = out.lock().unwrap().fraction;
        while let Some(left) = MIN_SHOWN.checked_sub(opened.elapsed()) {
            thread::sleep(TICK.min(left));
            let gone = opened.elapsed().as_secs_f32() / MIN_SHOWN.as_secs_f32();
            out.lock().unwrap().fraction = from + (1.0 - from) * gone.clamp(0.0, 1.0);
            ctx.request_repaint();
        }

        let mut p = out.lock().unwrap();
        p.version = version;
        p.encoders = encoders;
        p.fraction = 1.0;
        p.step = "Ready".into();
        p.ready = true;
        drop(p);
        ctx.request_repaint();
    });

    shared
}

fn fail(out: &Shared, ctx: &egui::Context, msg: &str) {
    let mut p = out.lock().unwrap();
    p.error = Some(msg.to_string());
    p.step = "Cannot start".into();
    p.fraction = 1.0;
    drop(p);
    ctx.request_repaint();
}

pub fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(bin))
            .find(|p| p.is_file())
    })
}

/// "ffmpeg version 7.1.1" off the first line of `ffmpeg -version`.
fn version() -> String {
    let Ok(out) = Command::new("ffmpeg").arg("-version").output() else {
        return "unknown".into();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("unknown")
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The encoder names we care about, picked out of `ffmpeg -encoders`.
///
/// Each line looks like ` V....D libx264   libx264 H.264 ...`, so the second
/// whitespace-separated field is the name.
fn encoders() -> HashSet<&'static str> {
    let Ok(out) = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
    else {
        return HashSet::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let present: HashSet<&str> = text
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect();
    TARGETS
        .iter()
        .map(|t| t.encoder())
        .filter(|name| present.contains(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{start, MIN_SHOWN, STEPS};
    use crate::preset::TARGETS;
    use std::time::{Duration, Instant};

    /// Walks the real start-up path against the ffmpeg on this machine, which
    /// is the only way to know the banner ever reaches "ready".
    #[test]
    fn start_up_finishes_and_reports_every_step() {
        if super::which("ffmpeg").is_none() {
            eprintln!("no ffmpeg here — skipping");
            return;
        }

        let ctx = egui::Context::default();
        let opened = Instant::now();
        let shared = start(&ctx);

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let p = shared.lock().unwrap().clone();
            assert_eq!(p.error, None, "start-up reported a problem");
            if p.ready {
                assert!(opened.elapsed() >= MIN_SHOWN, "the banner flashed past");
                assert_eq!(p.fraction, 1.0);
                assert_eq!(p.log.len(), STEPS - 1, "one line per completed step");
                assert!(p.version.starts_with("ffmpeg version"), "{}", p.version);
                // This machine's ffmpeg has all of them; a build missing one
                // would simply report a smaller set, never a larger one.
                assert!(p.encoders.len() <= TARGETS.len());
                assert!(p.encoders.contains("dnxhd"), "the default target's encoder");
                return;
            }
            assert!(Instant::now() < deadline, "start-up never finished");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
