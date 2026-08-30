//! Output targets and the ffmpeg argument lists they translate into.
//!
//! Every preset here is built around one rule: never resample, never resize,
//! never change the frame rate. 48p, 60p, 23.976p and anything else the source
//! carries is passed through untouched.

use crate::media::MediaInfo;
use std::ffi::OsString;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    DnxhrMov,
    ProresMov,
    Av1Mp4,
    Av1Mkv,
    H264Mp4,
    H265Mp4,
    UtvideoAvi,
    Ffv1Mkv,
}

pub const TARGETS: [Target; 8] = [
    Target::DnxhrMov,
    Target::ProresMov,
    Target::Av1Mp4,
    Target::Av1Mkv,
    Target::H264Mp4,
    Target::H265Mp4,
    Target::UtvideoAvi,
    Target::Ffv1Mkv,
];

impl Target {
    pub fn ext(self) -> &'static str {
        match self {
            Target::DnxhrMov | Target::ProresMov => "mov",
            Target::Av1Mp4 | Target::H264Mp4 | Target::H265Mp4 => "mp4",
            Target::Av1Mkv | Target::Ffv1Mkv => "mkv",
            Target::UtvideoAvi => "avi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::DnxhrMov => "DNxHR / MOV  —  DaVinci Resolve ready",
            Target::ProresMov => "ProRes / MOV  —  editing",
            Target::Av1Mp4 => "AV1 / MP4  —  small, modern",
            Target::Av1Mkv => "AV1 / MKV  —  small, keeps every track",
            Target::H264Mp4 => "H.264 / MP4  —  plays everywhere",
            Target::H265Mp4 => "H.265 / MP4  —  efficient",
            Target::UtvideoAvi => "Ut Video / AVI  —  lossless AVI",
            Target::Ffv1Mkv => "FFV1 / MKV  —  lossless archive",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Target::DnxhrMov => "Intra-frame DNxHR with PCM audio. This is the one that makes DaVinci Resolve on Linux behave: it accepts any resolution and any frame rate (48, 60, 23.976 …), scrubs instantly and has no H.264/AAC licensing gap.",
            Target::ProresMov => "Apple ProRes 422 HQ with PCM audio. Same idea as DNxHR, handy when the project also travels to macOS.",
            Target::Av1Mp4 => "SVT-AV1. Much smaller files at the same visual quality; slower to encode and heavy to scrub in an editor.",
            Target::Av1Mkv => "SVT-AV1 in Matroska, all audio/subtitle tracks copied through untouched.",
            Target::H264Mp4 => "x264. The safe delivery format for phones, browsers and social platforms.",
            Target::H265Mp4 => "x265. Roughly half the size of H.264 at equal quality, keeps 10-bit sources 10-bit.",
            Target::UtvideoAvi => "Mathematically lossless video in an AVI container with PCM audio. Big files, zero generation loss.",
            Target::Ffv1Mkv => "Mathematically lossless FFV1, the archival choice. Keeps the source pixel format bit for bit.",
        }
    }

    /// Intra-frame formats meant for a timeline: force constant frame rate so
    /// editors do not choke on variable-frame-rate phone footage.
    pub fn for_editing(self) -> bool {
        matches!(
            self,
            Target::DnxhrMov | Target::ProresMov | Target::UtvideoAvi | Target::Ffv1Mkv
        )
    }

    pub fn is_lossless(self) -> bool {
        matches!(self, Target::UtvideoAvi | Target::Ffv1Mkv)
    }

    pub fn uses_quality(self) -> bool {
        matches!(
            self,
            Target::Av1Mp4 | Target::Av1Mkv | Target::H264Mp4 | Target::H265Mp4
        )
    }

    pub fn uses_profile(self) -> bool {
        matches!(self, Target::DnxhrMov | Target::ProresMov)
    }

    /// The ffmpeg encoder this target needs, so start-up can tell whether the
    /// local build actually has it. Fedora's `ffmpeg-free`, for instance, has
    /// no x264 or x265.
    pub fn encoder(self) -> &'static str {
        match self {
            Target::DnxhrMov => "dnxhd",
            Target::ProresMov => "prores_ks",
            Target::Av1Mp4 | Target::Av1Mkv => "libsvtav1",
            Target::H264Mp4 => "libx264",
            Target::H265Mp4 => "libx265",
            Target::UtvideoAvi => "utvideo",
            Target::Ffv1Mkv => "ffv1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Lossless,
    VisuallyLossless,
    High,
    Balanced,
}

pub const QUALITIES: [Quality; 4] = [
    Quality::Lossless,
    Quality::VisuallyLossless,
    Quality::High,
    Quality::Balanced,
];

impl Quality {
    pub fn label(self) -> &'static str {
        match self {
            Quality::Lossless => "Lossless (identical pixels, huge)",
            Quality::VisuallyLossless => "Visually lossless (recommended)",
            Quality::High => "High",
            Quality::Balanced => "Balanced",
        }
    }
}

/// DNxHR / ProRes profile. `SQ`, `HQ` … are the source-material terms the
/// grading world uses; they are quality tiers, not resolutions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Sq,
    Hq,
    Hqx,
    Fourfourfour,
}

pub const PROFILES: [Profile; 4] = [
    Profile::Sq,
    Profile::Hq,
    Profile::Hqx,
    Profile::Fourfourfour,
];

impl Profile {
    pub fn label(self, target: Target) -> &'static str {
        match (target, self) {
            (Target::ProresMov, Profile::Sq) => "ProRes 422 (standard)",
            (Target::ProresMov, Profile::Hq) => "ProRes 422 HQ (recommended)",
            (Target::ProresMov, Profile::Hqx) => "ProRes 4444",
            (Target::ProresMov, Profile::Fourfourfour) => "ProRes 4444 XQ",
            (_, Profile::Sq) => "DNxHR SQ (8-bit, lighter)",
            (_, Profile::Hq) => "DNxHR HQ (8-bit, recommended)",
            (_, Profile::Hqx) => "DNxHR HQX (10-bit, HDR / log)",
            (_, Profile::Fourfourfour) => "DNxHR 444 (10-bit 4:4:4)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audio {
    Auto,
    Copy,
    Pcm16,
    Pcm24,
    Flac,
    Aac320,
    Opus192,
}

pub const AUDIOS: [Audio; 7] = [
    Audio::Auto,
    Audio::Copy,
    Audio::Pcm16,
    Audio::Pcm24,
    Audio::Flac,
    Audio::Aac320,
    Audio::Opus192,
];

impl Audio {
    pub fn label(self) -> &'static str {
        match self {
            Audio::Auto => "Auto (copy untouched when the container allows)",
            Audio::Copy => "Copy the original stream (no re-encode)",
            Audio::Pcm16 => "PCM 16-bit (uncompressed)",
            Audio::Pcm24 => "PCM 24-bit (uncompressed)",
            Audio::Flac => "FLAC (lossless, compressed)",
            Audio::Aac320 => "AAC 320 kbps",
            Audio::Opus192 => "Opus 192 kbps",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub target: Target,
    pub quality: Quality,
    pub profile: Profile,
    pub audio: Audio,
    pub keep_all_tracks: bool,
    pub overwrite: bool,
    pub concurrency: usize,
    pub extra_args: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target: Target::DnxhrMov,
            quality: Quality::VisuallyLossless,
            profile: Profile::Hq,
            audio: Audio::Auto,
            keep_all_tracks: true,
            overwrite: false,
            concurrency: 1,
            extra_args: String::new(),
        }
    }
}

fn pix_fmt(s: &Settings, info: &MediaInfo) -> Option<String> {
    let ten = info.is_10bit();
    Some(match s.target {
        Target::DnxhrMov => match s.profile {
            Profile::Sq | Profile::Hq => "yuv422p",
            Profile::Hqx => "yuv422p10le",
            Profile::Fourfourfour => "yuv444p10le",
        }
        .into(),
        Target::ProresMov => match s.profile {
            Profile::Sq | Profile::Hq => "yuv422p10le",
            Profile::Hqx | Profile::Fourfourfour => "yuva444p10le",
        }
        .into(),
        Target::Av1Mp4 | Target::Av1Mkv => {
            if ten {
                "yuv420p10le".into()
            } else {
                "yuv420p".into()
            }
        }
        Target::H264Mp4 => {
            if [
                "yuv420p",
                "yuv422p",
                "yuv444p",
                "yuv420p10le",
                "yuv422p10le",
                "yuv444p10le",
            ]
            .contains(&info.pix_fmt.as_str())
            {
                info.pix_fmt.clone()
            } else if ten {
                "yuv420p10le".into()
            } else {
                "yuv420p".into()
            }
        }
        Target::H265Mp4 => {
            if [
                "yuv420p",
                "yuv422p",
                "yuv444p",
                "yuv420p10le",
                "yuv422p10le",
                "yuv444p10le",
            ]
            .contains(&info.pix_fmt.as_str())
            {
                info.pix_fmt.clone()
            } else if ten {
                "yuv420p10le".into()
            } else {
                "yuv420p".into()
            }
        }
        Target::UtvideoAvi => {
            if ["yuv420p", "yuv422p", "yuv444p", "gbrp"].contains(&info.pix_fmt.as_str()) {
                info.pix_fmt.clone()
            } else {
                "yuv422p".into()
            }
        }
        // FFV1 speaks nearly every pixel format: leave the source alone.
        Target::Ffv1Mkv => return None,
    })
}

fn video_args(s: &Settings, info: &MediaInfo) -> Vec<String> {
    let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match s.target {
        Target::DnxhrMov => {
            let p = match s.profile {
                Profile::Sq => "dnxhr_sq",
                Profile::Hq => "dnxhr_hq",
                Profile::Hqx => "dnxhr_hqx",
                Profile::Fourfourfour => "dnxhr_444",
            };
            a(&["-c:v", "dnxhd", "-profile:v", p])
        }
        Target::ProresMov => {
            let p = match s.profile {
                Profile::Sq => "2",
                Profile::Hq => "3",
                Profile::Hqx => "4",
                Profile::Fourfourfour => "5",
            };
            a(&[
                "-c:v",
                "prores_ks",
                "-profile:v",
                p,
                "-vendor",
                "apl0",
                "-qscale:v",
                "4",
            ])
        }
        Target::Av1Mp4 | Target::Av1Mkv => {
            let crf = match s.quality {
                Quality::Lossless => "0",
                Quality::VisuallyLossless => "18",
                Quality::High => "24",
                Quality::Balanced => "30",
            };
            let mut v = a(&["-c:v", "libsvtav1", "-preset", "6", "-crf", crf]);
            v.extend(a(&["-svtav1-params", "tune=0:film-grain=0"]));
            v
        }
        Target::H264Mp4 => {
            let crf = match s.quality {
                Quality::Lossless => "0",
                Quality::VisuallyLossless => "16",
                Quality::High => "18",
                Quality::Balanced => "21",
            };
            a(&["-c:v", "libx264", "-preset", "slow", "-crf", crf])
        }
        Target::H265Mp4 => {
            let crf = match s.quality {
                Quality::Lossless => "0",
                Quality::VisuallyLossless => "18",
                Quality::High => "20",
                Quality::Balanced => "23",
            };
            let mut v = a(&["-c:v", "libx265", "-preset", "slow", "-crf", crf]);
            if s.quality == Quality::Lossless {
                v.extend(a(&["-x265-params", "lossless=1"]));
            }
            v.extend(a(&["-tag:v", "hvc1"]));
            v
        }
        Target::UtvideoAvi => a(&["-c:v", "utvideo", "-pred", "median"]),
        Target::Ffv1Mkv => a(&[
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-coder",
            "1",
            "-context",
            "1",
            "-g",
            "1",
            "-slices",
            "24",
            "-slicecrc",
            "1",
        ]),
    }
    .into_iter()
    .chain(
        // Keep the colour description of the source; a wrong tag is the most
        // common cause of "the colours shifted after converting".
        [
            info.color_primaries
                .as_ref()
                .map(|v| ("-color_primaries", v)),
            info.color_transfer.as_ref().map(|v| ("-color_trc", v)),
            info.color_space.as_ref().map(|v| ("-colorspace", v)),
            info.color_range.as_ref().map(|v| ("-color_range", v)),
        ]
        .into_iter()
        .flatten()
        .flat_map(|(k, v)| [k.to_string(), v.clone()]),
    )
    .collect()
}

fn audio_args(s: &Settings, info: &MediaInfo) -> Vec<String> {
    if info.audio_codec.is_none() {
        return vec!["-an".into()];
    }
    let src = info.audio_codec.clone().unwrap_or_default();
    let mode = match s.audio {
        Audio::Auto => match s.target {
            // Resolve on Linux will not open AAC; uncompressed always works.
            Target::DnxhrMov | Target::ProresMov | Target::UtvideoAvi => {
                if src.starts_with("pcm_") {
                    Audio::Copy
                } else {
                    Audio::Pcm24
                }
            }
            Target::Ffv1Mkv => {
                if src.starts_with("pcm_") || src == "flac" || src == "alac" {
                    Audio::Copy
                } else {
                    Audio::Flac
                }
            }
            Target::Av1Mkv => Audio::Copy,
            Target::Av1Mp4 | Target::H264Mp4 | Target::H265Mp4 => {
                if ["aac", "mp3", "ac3", "eac3", "alac", "opus"].contains(&src.as_str()) {
                    Audio::Copy
                } else {
                    Audio::Aac320
                }
            }
        },
        other => other,
    };
    let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match mode {
        Audio::Copy | Audio::Auto => a(&["-c:a", "copy"]),
        Audio::Pcm16 => a(&["-c:a", "pcm_s16le"]),
        Audio::Pcm24 => a(&["-c:a", "pcm_s24le"]),
        Audio::Flac => a(&["-c:a", "flac"]),
        Audio::Aac320 => a(&["-c:a", "aac", "-b:a", "320k"]),
        Audio::Opus192 => a(&["-c:a", "libopus", "-b:a", "192k"]),
    }
}

/// The full ffmpeg invocation for one job.
pub fn build(input: &Path, output: &Path, info: &MediaInfo, s: &Settings) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |v: &str| args.push(OsString::from(v));

    for a in [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-y",
    ] {
        push(a);
    }
    args.push("-i".into());
    args.push(input.into());

    // Stream selection: first video track, every audio track, subtitles only
    // where the container can carry them.
    args.push("-map".into());
    args.push("0:v:0".into());
    if s.keep_all_tracks {
        args.push("-map".into());
        args.push("0:a?".into());
        if matches!(s.target, Target::Av1Mkv | Target::Ffv1Mkv) {
            args.push("-map".into());
            args.push("0:s?".into());
            args.push("-c:s".into());
            args.push("copy".into());
        }
    } else {
        args.push("-map".into());
        args.push("0:a:0?".into());
    }
    args.push("-map_metadata".into());
    args.push("0".into());

    for a in video_args(s, info) {
        args.push(a.into());
    }

    if let Some(pf) = pix_fmt(s, info) {
        // Chroma-subsampled formats need even dimensions; pad rather than scale
        // so not a single pixel of the picture is resampled.
        let sub420 = pf.starts_with("yuv420");
        let sub422 = pf.starts_with("yuv422");
        let odd_w = info.width % 2 == 1;
        let odd_h = info.height % 2 == 1;
        if (sub420 && (odd_w || odd_h)) || (sub422 && odd_w) {
            args.push("-vf".into());
            args.push("pad=ceil(iw/2)*2:ceil(ih/2)*2".into());
        }
        args.push("-pix_fmt".into());
        args.push(pf.into());
    }

    args.push("-fps_mode".into());
    args.push(if s.target.for_editing() {
        "cfr".into()
    } else {
        "passthrough".into()
    });

    for a in audio_args(s, info) {
        args.push(a.into());
    }

    if matches!(s.target, Target::Av1Mp4 | Target::H264Mp4 | Target::H265Mp4) {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }

    for a in s.extra_args.split_whitespace() {
        args.push(a.into());
    }

    args.push(output.into());
    args
}

/// Same arguments, rendered as a copy-pasteable shell command.
pub fn preview(input: &Path, output: &Path, info: &MediaInfo, s: &Settings) -> String {
    let quote = |v: &OsString| {
        let t = v.to_string_lossy().to_string();
        if t.contains(' ') {
            format!("'{t}'")
        } else {
            t
        }
    };
    let args: Vec<String> = build(input, output, info, s).iter().map(quote).collect();
    format!("ffmpeg {}", args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> MediaInfo {
        MediaInfo {
            duration: 10.0,
            width: 1920,
            height: 1080,
            fps: 47.952,
            codec: "h264".into(),
            pix_fmt: "yuv420p".into(),
            audio_codec: Some("aac".into()),
            color_space: Some("bt709".into()),
            ..Default::default()
        }
    }

    fn args_of(target: Target) -> Vec<String> {
        let mut s = Settings::default();
        s.target = target;
        build(
            Path::new("/in/clip.mov"),
            Path::new("/out/clip.x"),
            &info(),
            &s,
        )
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect()
    }

    /// Nothing may resize or resample: no -s, no -r, no scale filter, ever.
    #[test]
    fn never_touches_geometry_or_frame_rate() {
        for t in TARGETS {
            let args = args_of(t);
            assert!(!args.iter().any(|a| a == "-s" || a == "-r"), "{t:?}");
            assert!(
                !args
                    .iter()
                    .any(|a| a.contains("scale=") || a.contains("fps=")),
                "{t:?}"
            );
        }
    }

    #[test]
    fn colour_tags_follow_the_source() {
        let args = args_of(Target::H264Mp4);
        let i = args.iter().position(|a| a == "-colorspace").unwrap();
        assert_eq!(args[i + 1], "bt709");
    }

    #[test]
    fn resolve_preset_uses_dnxhr_and_uncompressed_audio() {
        let args = args_of(Target::DnxhrMov);
        assert!(args.windows(2).any(|w| w == ["-c:v", "dnxhd"]));
        assert!(args.windows(2).any(|w| w == ["-profile:v", "dnxhr_hq"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "pcm_s24le"]));
        assert!(args.windows(2).any(|w| w == ["-fps_mode", "cfr"]));
    }

    #[test]
    fn already_compatible_audio_is_copied_not_re_encoded() {
        let args = args_of(Target::H264Mp4);
        assert!(args.windows(2).any(|w| w == ["-c:a", "copy"]));
    }

    #[test]
    fn odd_dimensions_are_padded_never_scaled() {
        let mut s = Settings::default();
        s.target = Target::H264Mp4;
        let mut i = info();
        i.width = 641;
        i.height = 361;
        let args: Vec<String> = build(Path::new("/in/a.mp4"), Path::new("/out/a.mp4"), &i, &s)
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.iter().any(|a| a.starts_with("pad=")));
    }
}
