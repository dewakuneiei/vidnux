//! Source inspection via `ffprobe`.

use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
    pub pix_fmt: String,
    pub audio_codec: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub color_range: Option<String>,
}

impl MediaInfo {
    /// "1920x1080 · 59.94 fps · h264" — the badge shown in the queue.
    pub fn summary(&self) -> String {
        let res = match (self.width, self.height) {
            (0, _) | (_, 0) => "?".to_string(),
            (w, h) => format!("{w}x{h}"),
        };
        format!("{res} · {:.3} fps · {}", self.fps, self.codec).replace(".000 fps", " fps")
    }

    pub fn class(&self) -> &'static str {
        match self.height {
            0 => "",
            h if h >= 2000 => "4K+",
            h if h >= 1000 => "FullHD",
            h if h >= 700 => "HD",
            _ => "SD",
        }
    }

    pub fn is_10bit(&self) -> bool {
        self.pix_fmt.contains("10") || self.pix_fmt.contains("12") || self.pix_fmt.contains("16")
    }
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
}

fn ratio(s: &str) -> f64 {
    let mut it = s.split('/');
    let n: f64 = it.next().unwrap_or("0").parse().unwrap_or(0.0);
    let d: f64 = it.next().unwrap_or("1").parse().unwrap_or(1.0);
    if d == 0.0 {
        0.0
    } else {
        n / d
    }
}

fn tag(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty() && s != "unknown" && s != "reserved")
}

pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe could not be started: {e}"))?;

    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }

    let probe: Probe = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("unreadable ffprobe output: {e}"))?;

    let mut info = MediaInfo::default();
    info.duration = probe
        .format
        .and_then(|f| f.duration)
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);

    let mut has_video = false;
    for s in probe.streams {
        match s.codec_type.as_deref() {
            Some("video") if !has_video => {
                has_video = true;
                info.width = s.width.unwrap_or(0);
                info.height = s.height.unwrap_or(0);
                info.codec = s.codec_name.clone().unwrap_or_default();
                info.pix_fmt = s.pix_fmt.clone().unwrap_or_default();
                info.fps = s
                    .r_frame_rate
                    .as_deref()
                    .map(ratio)
                    .filter(|f| *f > 0.0)
                    .or_else(|| s.avg_frame_rate.as_deref().map(ratio))
                    .unwrap_or(0.0);
                info.color_space = tag(s.color_space);
                info.color_transfer = tag(s.color_transfer);
                info.color_primaries = tag(s.color_primaries);
                info.color_range = tag(s.color_range);
                if info.duration == 0.0 {
                    info.duration = s.duration.and_then(|d| d.parse().ok()).unwrap_or(0.0);
                }
            }
            Some("audio") if info.audio_codec.is_none() => {
                info.audio_codec = s.codec_name;
            }
            _ => {}
        }
    }

    if !has_video {
        return Err("no video stream found".into());
    }
    Ok(info)
}
