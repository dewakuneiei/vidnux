# Vidnux

A small, clean desktop app for converting video **without losing quality** — built with Rust + egui, driving ffmpeg underneath.

[**Handbook →**](https://raw.githack.com/dewakuneiei/vidnux/main/docs/index.html) · made by [dewakuneiei](https://github.com/dewakuneiei) · [YouTube](https://www.youtube.com/@dewakuneiei) · [dewakuneiei.com](https://www.dewakuneiei.com)

Drop in as many files as you like, rename each output individually, pick a format, press start. The queue does the rest.

> Made for the case where DaVinci Resolve on Linux refuses to open your footage: the default preset (**DNxHR / MOV**) is the one Resolve always accepts, at any resolution and any frame rate — 23.976, 24, 25, 30, 48, 50, 60 fps all pass through untouched.

---

## Requirements

| Needed | Why |
| --- | --- |
| `ffmpeg` and `ffprobe` | all decoding/encoding |
| `cargo` (Rust 1.75+) | to build the app |

**Fedora / Nobara** (what this project is developed on — Nobara already ships the full ffmpeg):

```bash
sudo dnf install ffmpeg rust cargo
```

Other distributions:

```bash
# Debian / Ubuntu / Linux Mint / Pop!_OS
sudo apt update && sudo apt install ffmpeg cargo build-essential pkg-config libgtk-3-dev

# Arch / Manjaro / EndeavourOS
sudo pacman -S ffmpeg rust gtk3 base-devel

# openSUSE Tumbleweed / Leap
sudo zypper install ffmpeg rust cargo gtk3-devel

# Alpine
sudo apk add ffmpeg rust cargo gtk+3.0-dev build-base

# Any distro, if the packaged Rust is too old
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Notes per family:

- **Fedora / RHEL / Rocky**: plain Fedora ships `ffmpeg-free`, which lacks x264/x265. Enable RPM Fusion once and install the full build: `sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm && sudo dnf swap ffmpeg-free ffmpeg --allowerasing`. Nobara needs none of this.
- **Debian / Ubuntu**: `libgtk-3-dev` is what the file-picker dialog builds against.
- **Rust from rustup** lives in `~/.cargo/bin`; the installer looks there by itself even when that folder is not on your PATH.

---

## Install

```bash
./install.sh
```

That builds the release binary and installs, for your user only:

- `~/.local/bin/vidnux` — the app
- `~/.local/share/applications/vidnux.desktop` — app-menu entry
- `~/.local/share/icons/hicolor/scalable/apps/vidnux.svg` — icon

Then start it from the application menu, or:

```bash
vidnux
```

System-wide instead (all users, into `/usr/local`):

```bash
sudo ./install.sh --system
```

The installer is distribution-agnostic: it writes only to standard freedesktop locations, so the same two commands work on Fedora, Ubuntu, Debian, Arch, openSUSE, Mint and anything else with a normal desktop.

## Uninstall

```bash
./uninstall.sh            # removes binary, menu entry and icon
./uninstall.sh --purge    # also deletes the build cache (target/)

sudo ./uninstall.sh --system   # if it was installed with --system
```

Or by hand, if you no longer have the project folder:

```bash
rm -f ~/.local/bin/vidnux \
      ~/.local/share/applications/vidnux.desktop \
      ~/.local/share/icons/hicolor/scalable/apps/vidnux.svg
update-desktop-database ~/.local/share/applications
```

Converted videos are never touched by the uninstaller.

---

## Using it

When you launch it, Vidnux opens a start-up banner — its own window, its own size, no title bar — while it checks that ffmpeg and ffprobe are there and reads which encoders your build actually has. Each check is named as it happens, and the answers stay on screen. Once the checks are done the window grows into the interface. If ffmpeg is missing, the banner says so and gives you the install command for your distribution instead of failing on a terminal you may never see.

The checks take about a third of a second on a warm cache, so the banner is held to two seconds in total — long enough to read. That is padding only when there is time to pad: a machine that takes longer than two seconds to answer has already earned the wait, and nothing is added on top of it.

Output formats your ffmpeg cannot produce are greyed out in the format list. On plain Fedora, for instance, `ffmpeg-free` ships without x264 and x265, so `H.264 / MP4` and `H.265 / MP4` are unavailable until you swap in the full build (see Requirements).

1. **Add files** — “＋ Add files…” (select as many as you want with Ctrl/Shift), “📁 Add folder…”, or just **drag and drop** onto the window.
2. **Choose the output format** in the left panel. Hover the box for the full explanation of what the selected format is for.
3. **Rename** — every row has its own editable output name. `fileA.mp4` → type `Interview_take1` and it is written as `Interview_take1.mov`. The original file is never renamed or modified.
4. **Destination** — “next to each source file” by default, or pick one folder for everything.
5. **Sort the queue** with the `Sort: A-Z` button at the bottom when the order gets messy — press it again for `Z-A`. It is number-aware, so `clip2` comes before `clip10`. Individual rows can still be nudged with `^` and `v`.
6. **Start converting.** Each row shows live progress and encoding speed; a job can be cancelled, reordered (↑ ↓), removed or requeued at any time. “Run at once” converts 2–8 files in parallel if your CPU has the cores.

The queue scrolls, so the list is never limited to what happens to fit on screen, and the rows reflow with the window instead of running off the right edge. The settings panel on the left scrolls too, so every control stays reachable however short you make the window. Nudging a row with `^` or `v` takes effect at once — no animation — and the row you are moving is kept in view.

The strip along the bottom carries the queue tally over the renaming tools and one large **Start converting** button. Per-file progress lives on the rows themselves, where it belongs.

The **?** button at the top right opens the [handbook](https://raw.githack.com/dewakuneiei/vidnux/main/docs/index.html) — what Vidnux does to your footage and why, in more detail than this file goes into.

### Light and dark

Vidnux follows the desktop. It asks the XDG desktop portal — the same thing GNOME, KDE, Cinnamon, XFCE and Sway answer — and falls back to GNOME's `gsettings` and then `GTK_THEME`. Flip your desktop between light and dark and the app follows immediately, without a restart.

**If none of them answer, the app stays light.** The button on the right of the title bar cycles *system → light → dark* when you want to override that; hover it to see what the desktop actually reported.

### Batch renaming

The bar at the bottom applies a pattern to every row at once:

| Token | Becomes |
| --- | --- |
| `{name}` | the source file name without extension |
| `{n}` | position in the queue (`01`, `02`, …) |
| `{res}` | picture height, e.g. `1080p` |
| `{fps}` | frame rate, e.g. `48fps` |

Example: `{n}_{name}_{res}` → `01_fileA_1080p.mov`, `02_fileB_1080p.mov`. You can still fine-tune any single row afterwards.

**Names never collide.** Type a plain `Adam-Kun` and apply it to a queue of five files and you get `Adam-Kun-1` … `Adam-Kun-5`, because one name cannot serve five files. If you then edit rows by hand and two end up identical again, the converter settles it while writing: the first file keeps `Adam-Kun.mov`, the next becomes `Adam-Kun(1).mov`, then `Adam-Kun(2).mov`, and the row updates to show the name the file really got. The same happens when a file of that name already exists in the destination folder — nothing is overwritten unless you tick *Overwrite existing files*.

---

## Output formats

| Format | Codec | Use it for |
| --- | --- | --- |
| **DNxHR / MOV** *(default)* | `dnxhd` + PCM audio | **DaVinci Resolve**, Kdenlive, Premiere. Any resolution, any frame rate. |
| ProRes / MOV | `prores_ks` 422 HQ + PCM | same idea, when the project also goes to macOS |
| AV1 / MP4 · AV1 / MKV | `libsvtav1` | smallest modern delivery files |
| H.264 / MP4 | `libx264` | plays on literally everything |
| H.265 / MP4 | `libx265` | ~half the size of H.264, keeps 10-bit sources 10-bit |
| Ut Video / AVI | `utvideo` + PCM | a truly lossless **AVI** |
| FFV1 / MKV | `ffv1` | lossless archival master |

Inputs: anything ffmpeg can read — mp4, mov, mkv, avi, wmv, flv, mpg, mts/m2ts, ts, mxf, webm, 3gp, ogv, vob, dv, y4m, gif and more.

**Profiles** (DNxHR / ProRes): `SQ` → light, `HQ` → the recommended default, `HQX` → 10-bit for log/HDR footage, `444` → 10-bit 4:4:4 for keying and grading. These are quality tiers, not resolutions; SD, HD, Full HD and 4K all work with each of them.

**Quality** (AV1 / H.264 / H.265): *Lossless*, *Visually lossless* (default), *High*, *Balanced*.

---

## Why the result does not lose quality

*ทำไมไฟล์ที่ได้ไม่เสียคุณภาพจากของเดิม*

- **Resolution is never changed.** No scaling filter is ever added. If a source has odd dimensions it is *padded* by one pixel rather than resampled, so no existing pixel is ever recomputed. (There is a test enforcing this.)
- **Frame rate is never changed.** 60, 50, 48, 30, 25, 24 and 23.976 fps all come out exactly as they went in. No `-r`, no `fps` filter. Editing formats are written as constant frame rate so timelines stay in sync with variable-frame-rate phone recordings.
- **Colour is carried over.** Primaries, transfer, matrix and range are read from the source with `ffprobe` and written to the output, so nothing shifts to washed-out or over-saturated.
- **Bit depth is kept.** A 10-bit source stays 10-bit (except where a codec cannot, and then the app picks the closest format it can).
- **Audio is copied untouched** whenever the container allows it. When it cannot be copied it goes to PCM/FLAC (lossless) — never to a lower bitrate than needed.
- **Every track is kept** — all audio tracks, plus subtitles in MKV, plus file metadata.
- With **Ut Video / AVI** or **FFV1 / MKV** the video is *mathematically* identical to the source, pixel for pixel.

One honest caveat: DNxHR, ProRes, AV1, H.264 and H.265 are compressed formats, so at the default *Visually lossless* setting the result is not bit-identical to the source — it is indistinguishable by eye and safe for further editing and grading. If you need bit-identical output, use FFV1/MKV or Ut Video/AVI (much larger files), or the *Lossless* quality setting for H.264/H.265/AV1.

Curious what is actually being run? Left panel → **Advanced** → *Show the ffmpeg command per file*.

---

## DaVinci Resolve notes

The free version of Resolve on Linux has no H.264/H.265 decoder and no AAC decoder, which is why most phone and camera files show up as offline, audio-less, or refuse to import at all. Converting to **DNxHR / MOV with PCM audio** removes both problems, and unlike DNxHD, DNxHR has no fixed table of allowed resolutions and frame rates — 48 fps and 60 fps footage is fine.

Recommended: **DNxHR HQ** for normal 8-bit footage, **DNxHR HQX** for 10-bit / log / HDR footage.

---

## Development

```bash
cargo run          # debug build
cargo test         # preset/argument tests
cargo build --release
```

| File | Contents |
| --- | --- |
| `src/media.rs` | ffprobe inspection |
| `src/preset.rs` | output formats → ffmpeg arguments (+ tests) |
| `src/job.rs` | queue, worker pool, progress parsing |
| `src/boot.rs` | the start-up checks the banner reports (+ tests) |
| `src/splash.rs` | the start-up banner |
| `src/theme.rs` | reading the desktop's light / dark preference (+ tests) |
| `src/app.rs` | the egui interface |
| `assets/splash.svg` | banner artwork, compiled into the binary |
| `assets/pandora_headshot.svg` | author's mark on the banner |
| `docs/index.html` | the handbook the **?** button opens |

## Troubleshooting

- **The banner says ffmpeg was not found** — install it (see Requirements). The banner shows the command for the common distributions.
- **A format is greyed out** — your ffmpeg was built without that encoder; the banner's `encoders` line names which ones are missing.
- **`./install.sh` says `Missing: cargo` although Rust is installed** — rustup puts cargo in `~/.cargo/bin`, which is missing from the PATH of that shell (this is what `sudo` does too). The installer now looks there by itself; if it still cannot find it, run `export PATH="$HOME/.cargo/bin:$PATH"` and try again.
- **`vidnux: command not found`** — add `export PATH="$HOME/.local/bin:$PATH"` to `~/.bashrc` and open a new terminal.
- **Encoding feels slow** — DNxHR and lossless formats are fast but write big files; AV1 is the slowest. Raise “Run at once” to use more cores.
- **The app is light but my desktop is dark** — nothing answered the theme lookup, so it fell back to light. Hover the theme button in the title bar to see what was found, and press it to pick light or dark yourself. Installing `xdg-desktop-portal` for your desktop usually fixes the detection.

---

## Licence

MIT — see [LICENSE](LICENSE). Clone it, change it, ship it, sell it; keep the copyright line and you are done.

Made by [dewakuneiei](https://github.com/dewakuneiei) · [YouTube](https://www.youtube.com/@dewakuneiei) · [dewakuneiei.com](https://www.dewakuneiei.com)
