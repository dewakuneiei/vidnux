//! Following the desktop's light / dark preference.
//!
//! egui only learns the system theme when the windowing backend reports one,
//! and on Linux (X11, and most Wayland compositors under winit 0.30) it never
//! does. So we ask the desktop ourselves — first the XDG desktop portal, which
//! every modern desktop answers, then GNOME's `gsettings`, then `GTK_THEME`.
//!
//! When nothing answers, or every lookup fails, the app stays **Light**.

use egui::{Color32, Theme};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// What the user picked in the title bar, which is not always what is drawn:
/// `Auto` resolves to whatever the desktop reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Auto,
    Light,
    Dark,
}

impl Mode {
    /// Short enough to sit in the tool bar of a narrow window; the button's
    /// hover text carries the explanation.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Auto => "System",
            Mode::Light => "Light",
            Mode::Dark => "Dark",
        }
    }

    /// Cycle system -> light -> dark -> system.
    pub fn next(self) -> Self {
        match self {
            Mode::Auto => Mode::Light,
            Mode::Light => Mode::Dark,
            Mode::Dark => Mode::Auto,
        }
    }
}

/// The desktop's answer, kept fresh by a background watcher.
#[derive(Clone, Default)]
pub struct Watcher {
    /// 0 = not detected, 1 = light, 2 = dark. An atomic keeps the read side
    /// free of locking, since the GUI reads it once per frame.
    theme: Arc<AtomicU8>,
    /// The `gdbus monitor` we are listening to, so a graceful quit can take it
    /// down. 0 means there is none.
    monitor: Arc<AtomicU32>,
}

impl Watcher {
    /// Read the desktop preference once, then keep watching for changes.
    ///
    /// `gdbus monitor` turns the watch into a single long-lived process that
    /// only speaks when the setting actually changes; without it we fall back
    /// to a slow poll. Either way the app repaints as soon as the theme flips,
    /// so switching the desktop to dark mode changes Vidnux immediately.
    ///
    /// The monitor never writes unless the theme changes, so it would never
    /// notice our end of the pipe closing — it has to be tied to our lifetime
    /// explicitly, see [`DieWithUs`].
    pub fn spawn(ctx: &egui::Context) -> Self {
        let watcher = Self::default();
        watcher.store(detect());

        let out = watcher.theme.clone();
        let monitor_pid = watcher.monitor.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let monitor = Command::new("gdbus")
                .args([
                    "monitor",
                    "--session",
                    "--dest",
                    "org.freedesktop.portal.Desktop",
                    "--object-path",
                    "/org/freedesktop/portal/desktop",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .die_with_us()
                .spawn();

            let apply = |out: &AtomicU8, ctx: &egui::Context| {
                let fresh = encode(detect());
                if out.swap(fresh, Ordering::Relaxed) != fresh {
                    ctx.request_repaint();
                }
            };

            if let Ok(mut child) = monitor {
                monitor_pid.store(child.id(), Ordering::Relaxed);
                if let Some(stdout) = child.stdout.take() {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if line.contains("color-scheme") {
                            apply(&out, &ctx);
                        }
                    }
                }
                let _ = child.wait();
                monitor_pid.store(0, Ordering::Relaxed);
            }

            // No portal to listen to: check now and then instead.
            loop {
                thread::sleep(Duration::from_secs(3));
                apply(&out, &ctx);
            }
        });

        watcher
    }

    pub fn get(&self) -> Option<Theme> {
        match self.theme.load(Ordering::Relaxed) {
            1 => Some(Theme::Light),
            2 => Some(Theme::Dark),
            _ => None,
        }
    }

    /// Take the monitor down on a graceful quit. The kernel handles every
    /// other way the app can end, see [`DieWithUs`].
    pub fn stop(&self) {
        let pid = self.monitor.swap(0, Ordering::Relaxed);
        if pid != 0 {
            crate::job::kill(pid);
        }
    }

    fn store(&self, theme: Option<Theme>) {
        self.theme.store(encode(theme), Ordering::Relaxed);
    }
}

/// Ask the kernel to kill a child process as soon as the thread that spawned
/// it goes away — which for the watcher thread means "when Vidnux exits",
/// however it exits, `SIGKILL` included. Without it a stale `gdbus monitor`
/// would be reparented to init and linger after every run.
trait DieWithUs {
    fn die_with_us(&mut self) -> &mut Command;
}

impl DieWithUs for Command {
    fn die_with_us(&mut self) -> &mut Command {
        use std::os::unix::process::CommandExt;
        // SAFETY: `prctl` is async-signal-safe, which is all that is allowed
        // between fork and exec, and it touches only the new child.
        unsafe {
            self.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            })
        }
    }
}

fn encode(theme: Option<Theme>) -> u8 {
    match theme {
        Some(Theme::Light) => 1,
        Some(Theme::Dark) => 2,
        None => 0,
    }
}

/// Ask the desktop what it prefers. `None` means "could not tell", which the
/// caller turns into Light.
pub fn detect() -> Option<Theme> {
    portal().or_else(gsettings).or_else(gtk_env)
}

/// `org.freedesktop.appearance color-scheme`: 0 = no preference, 1 = dark,
/// 2 = light. Answered by GNOME, KDE, Cinnamon, XFCE, Sway … through their
/// portal implementation.
fn portal() -> Option<Theme> {
    let out = run(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.ReadOne",
            "org.freedesktop.appearance",
            "color-scheme",
        ],
    )
    // Portals older than 1.17 only have the plural `Read`.
    .or_else(|| {
        run(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.freedesktop.portal.Desktop",
                "--object-path",
                "/org/freedesktop/portal/desktop",
                "--method",
                "org.freedesktop.portal.Settings.Read",
                "org.freedesktop.appearance",
                "color-scheme",
            ],
        )
    })?;

    parse_color_scheme(&out)
}

/// `gdbus` prints the variant it got back: `(<uint32 1>,)` from `ReadOne`,
/// `(<<uint32 1>>,)` from the older `Read`. Either way the digit right after
/// the type name is the answer — 0 no preference, 1 dark, 2 light.
fn parse_color_scheme(out: &str) -> Option<Theme> {
    match out.split("uint32").nth(1)?.trim().chars().next()? {
        '1' => Some(Theme::Dark),
        '2' => Some(Theme::Light),
        _ => None,
    }
}

fn gsettings() -> Option<Theme> {
    let scheme = run(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "color-scheme"],
    )?;
    if scheme.contains("prefer-dark") {
        return Some(Theme::Dark);
    }
    if scheme.contains("prefer-light") {
        return Some(Theme::Light);
    }
    // "default" says nothing, so read it off the GTK theme name instead.
    let name = run(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "gtk-theme"],
    )?;
    Some(if is_dark_name(&name) {
        Theme::Dark
    } else {
        Theme::Light
    })
}

fn gtk_env() -> Option<Theme> {
    let name = std::env::var("GTK_THEME").ok()?;
    is_dark_name(&name).then_some(Theme::Dark)
}

fn is_dark_name(name: &str) -> bool {
    let name = name.to_lowercase();
    name.contains("dark") || name.ends_with(":dark")
}

fn run(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The handful of colours that have to change with the theme: the same accent
/// blue that reads well on charcoal turns invisible on white.
pub struct Palette {
    pub accent: Color32,
    /// Text drawn on top of `accent`.
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
    /// Background of the settings panel and the status strip. A hair away from
    /// the queue's own background, so the two areas separate by surface rather
    /// than by the hard rule egui would otherwise draw between them.
    pub surface: Color32,
    /// Background of a queue row, and of one under the pointer.
    pub row: Color32,
    pub row_hover: Color32,
    pub row_stroke: Color32,
}

pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Dark => Palette {
            accent: Color32::from_rgb(122, 162, 247),
            on_accent: Color32::from_rgb(16, 18, 24),
            ok: Color32::from_rgb(126, 200, 140),
            warn: Color32::from_rgb(230, 180, 90),
            bad: Color32::from_rgb(232, 118, 118),
            surface: Color32::from_rgb(24, 26, 31),
            row: Color32::from_rgb(32, 34, 40),
            row_hover: Color32::from_rgb(40, 43, 51),
            row_stroke: Color32::from_rgb(58, 62, 72),
        },
        Theme::Light => Palette {
            accent: Color32::from_rgb(38, 100, 214),
            on_accent: Color32::WHITE,
            ok: Color32::from_rgb(26, 130, 66),
            warn: Color32::from_rgb(150, 95, 10),
            bad: Color32::from_rgb(190, 44, 44),
            surface: Color32::from_rgb(242, 243, 246),
            row: Color32::from_rgb(252, 252, 253),
            row_hover: Color32::from_rgb(243, 245, 249),
            row_stroke: Color32::from_rgb(210, 214, 222),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{is_dark_name, parse_color_scheme};
    use egui::Theme;

    #[test]
    fn portal_replies_are_read_from_either_method() {
        // ReadOne
        assert_eq!(parse_color_scheme("(<uint32 1>,)"), Some(Theme::Dark));
        assert_eq!(parse_color_scheme("(<uint32 2>,)"), Some(Theme::Light));
        // The older plural Read wraps the variant one level deeper.
        assert_eq!(parse_color_scheme("(<<uint32 2>>,)"), Some(Theme::Light));
        // 0 means "no preference", which is not an answer.
        assert_eq!(parse_color_scheme("(<uint32 0>,)"), None);
        // Anything unexpected leaves the caller to fall through to the next
        // source rather than guessing.
        assert_eq!(parse_color_scheme("Error: no such interface"), None);
        assert_eq!(parse_color_scheme(""), None);
    }

    #[test]
    fn gtk_theme_names_give_away_dark_variants() {
        assert!(is_dark_name("'adw-gtk3-dark'"));
        assert!(is_dark_name("Adwaita:dark"));
        assert!(is_dark_name("Breeze-Dark"));
        assert!(!is_dark_name("'Adwaita'"));
        assert!(!is_dark_name("adw-gtk3"));
    }
}
