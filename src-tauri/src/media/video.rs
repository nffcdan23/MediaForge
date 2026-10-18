//! Rules for turning a downloaded video into one that opens in players and editing apps.
//!
//! The rules (which codecs need converting, how many bits to spend) are the Swift app's. What differs per
//! platform is the encoder: macOS has VideoToolbox; Windows PCs have one of NVIDIA NVENC, Intel Quick Sync or
//! AMD AMF, and every Windows 10/11 PC has Media Foundation's encoder as a fallback. None of these are
//! compiled into FFmpeg as encoder code, so the bundled FFmpeg stays an LGPL build.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::media::ffmpeg::{self, MediaProbe};
use crate::media::metadata::MediaFormat;
use crate::models::VideoCompatibility;
use crate::process::{self, ProcessRequest};

/// Video codecs that QuickTime, Windows' Media Player, Quick Look and the common editors read without help.
pub const PLAYABLE_VIDEO_CODECS: &[&str] = &["h264", "hevc"];
/// Audio codecs that are safe inside an MP4 on the same terms.
pub const PLAYABLE_AUDIO_CODECS: &[&str] = &["aac", "mp3", "alac", "ac3", "eac3"];

pub fn needs_conversion(probe: &MediaProbe) -> bool {
    probe.video.as_ref().is_some_and(|video| !PLAYABLE_VIDEO_CODECS.contains(&video.codec.to_lowercase().as_str()))
}

fn is_playable_codec_name(codec: &str) -> bool {
    let name = codec.to_lowercase();
    ["avc", "h264", "hev", "hvc", "h265"].iter().any(|prefix| name.starts_with(prefix))
}

/// Whether a download capped at `max_height` will arrive in a format that needs converting, judged from what
/// the source says it offers. Used to warn before the download starts.
pub fn expects_conversion(formats: &[MediaFormat], max_height: Option<u32>) -> bool {
    let video: Vec<&MediaFormat> = formats.iter().filter(|f| f.has_video() && f.height.is_some()).collect();
    let within: Vec<u32> = video.iter().filter_map(|f| f.height).filter(|h| max_height.is_none_or(|cap| *h <= cap)).collect();
    // The downloader takes the tallest stream within the cap, or failing that the shortest one above it.
    let Some(chosen) = within.iter().max().copied().or_else(|| video.iter().filter_map(|f| f.height).min()) else {
        return false;
    };
    let codecs: Vec<&str> = video.iter().filter(|f| f.height == Some(chosen)).filter_map(|f| f.video_codec.as_deref()).collect();
    !codecs.is_empty() && !codecs.iter().any(|c| is_playable_codec_name(c))
}

/// Bits per second to encode at.
///
/// Each resolution has a ceiling (the bitrates streaming services recommend for uploads). Within it the target
/// follows the source: hardware H.264 needs roughly two and a half times VP9's bitrate to look the same, and
/// there is no point spending more than that. HEVC needs about 60% of H.264's.
pub fn target_bitrate(height: Option<u32>, frame_rate: Option<f64>, source_bitrate: Option<u64>, target: VideoCompatibility) -> u64 {
    let high_frame_rate = frame_rate.unwrap_or(30.0) > 40.0;
    let ceiling: f64 = match height.unwrap_or(1080) {
        2000.. => {
            if high_frame_rate {
                68_000_000.0
            } else {
                45_000_000.0
            }
        }
        1300..=1999 => {
            if high_frame_rate {
                24_000_000.0
            } else {
                16_000_000.0
            }
        }
        900..=1299 => {
            if high_frame_rate {
                12_000_000.0
            } else {
                8_000_000.0
            }
        }
        600..=899 => {
            if high_frame_rate {
                7_500_000.0
            } else {
                5_000_000.0
            }
        }
        _ => {
            if high_frame_rate {
                4_000_000.0
            } else {
                2_500_000.0
            }
        }
    };
    let mut bitrate = ceiling * 0.75;
    if let Some(source) = source_bitrate.filter(|b| *b > 0) {
        bitrate = (source as f64 * 2.5).max(ceiling * 0.4).min(ceiling);
    }
    if target == VideoCompatibility::Hevc {
        bitrate *= 0.6;
    }
    bitrate.round() as u64
}

/// A hardware (or OS-provided) video encoder family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EncoderFamily {
    VideoToolbox,
    Nvenc,
    QuickSync,
    Amf,
    MediaFoundation,
}

impl EncoderFamily {
    pub fn encoder(self, target: VideoCompatibility) -> Option<&'static str> {
        let hevc = target == VideoCompatibility::Hevc;
        if target == VideoCompatibility::Original {
            return None;
        }
        Some(match (self, hevc) {
            (Self::VideoToolbox, false) => "h264_videotoolbox",
            (Self::VideoToolbox, true) => "hevc_videotoolbox",
            (Self::Nvenc, false) => "h264_nvenc",
            (Self::Nvenc, true) => "hevc_nvenc",
            (Self::QuickSync, false) => "h264_qsv",
            (Self::QuickSync, true) => "hevc_qsv",
            (Self::Amf, false) => "h264_amf",
            (Self::Amf, true) => "hevc_amf",
            (Self::MediaFoundation, false) => "h264_mf",
            (Self::MediaFoundation, true) => "hevc_mf",
        })
    }

    /// Encoders to try on this platform, best first.
    pub fn candidates() -> &'static [EncoderFamily] {
        if cfg!(target_os = "macos") {
            &[Self::VideoToolbox]
        } else if cfg!(windows) {
            &[Self::Nvenc, Self::QuickSync, Self::Amf, Self::MediaFoundation]
        } else {
            &[Self::Nvenc, Self::QuickSync]
        }
    }

    fn pixel_format(self) -> &'static str {
        // Quick Sync only takes NV12; everything else accepts planar 4:2:0, which every player handles.
        if self == Self::QuickSync { "nv12" } else { "yuv420p" }
    }

    fn rate_control(self, bitrate: u64, target: VideoCompatibility) -> Vec<String> {
        let b = bitrate.to_string();
        let max = (bitrate as f64 * 1.5).round().to_string();
        let buffer = (bitrate * 2).to_string();
        let mut arguments: Vec<String> = match self {
            // `allow_sw` lets the Mac fall back to software if the hardware is busy.
            Self::VideoToolbox => vec!["-b:v".into(), b, "-allow_sw".into(), "1".into()],
            Self::Nvenc => vec![
                "-preset".into(),
                "p5".into(),
                "-rc".into(),
                "vbr".into(),
                "-b:v".into(),
                b,
                "-maxrate".into(),
                max,
                "-bufsize".into(),
                buffer,
            ],
            Self::QuickSync => vec!["-preset".into(), "medium".into(), "-b:v".into(), b, "-maxrate".into(), max, "-bufsize".into(), buffer],
            Self::Amf => {
                vec!["-quality".into(), "balanced".into(), "-rc".into(), "vbr_peak".into(), "-b:v".into(), b, "-maxrate".into(), max]
            }
            // Windows' own encoder: present on every Windows 10/11 PC, used when no graphics encoder works.
            Self::MediaFoundation => vec!["-b:v".into(), b],
        };
        if target == VideoCompatibility::Hevc {
            // QuickTime and Apple devices only recognise HEVC in an MP4 under this tag.
            arguments.extend(["-tag:v".into(), "hvc1".into()]);
        } else if self != Self::MediaFoundation {
            arguments.extend(["-profile:v".into(), "high".into()]);
        }
        arguments
    }
}

pub fn arguments(input: &Path, output: &Path, probe: &MediaProbe, target: VideoCompatibility, family: EncoderFamily) -> Vec<OsString> {
    let Some(encoder) = family.encoder(target) else { return Vec::new() };
    let video = probe.video.as_ref();
    let bitrate = target_bitrate(video.and_then(|v| v.height), video.and_then(|v| v.frame_rate), video.and_then(|v| v.bitrate), target);
    let mut arguments = ffmpeg::video_preamble(input);
    let mut rest: Vec<String> = vec![
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(), // a silent video is still a video
        "-sn".into(),
        "-dn".into(),
        "-c:v".into(),
        encoder.into(),
        "-pix_fmt".into(),
        family.pixel_format().into(),
    ];
    rest.extend(family.rate_control(bitrate, target));
    if let Some(audio) = &probe.audio {
        if PLAYABLE_AUDIO_CODECS.contains(&audio.codec.to_lowercase().as_str()) {
            rest.extend(["-c:a".into(), "copy".into()]);
        } else {
            rest.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "256k".into()]);
        }
    }
    rest.extend(["-map_metadata".into(), "0".into(), "-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into()]);
    arguments.extend(rest.into_iter().map(OsString::from));
    arguments.push(ffmpeg::file_argument(output));
    arguments
}

/// Remembers which encoder works with a given FFmpeg on this computer, so the test encode runs once.
#[derive(Default)]
pub struct EncoderCache {
    chosen: Mutex<HashMap<(PathBuf, bool), Option<EncoderFamily>>>,
}

impl EncoderCache {
    /// The first encoder family that can actually produce frames here, or `None` if none can.
    pub async fn choose(&self, ffmpeg_path: &Path, target: VideoCompatibility, cancel: &CancellationToken) -> Option<EncoderFamily> {
        let candidates = EncoderFamily::candidates();
        if candidates.len() == 1 {
            // VideoToolbox is part of macOS and falls back to software itself.
            return Some(candidates[0]);
        }
        let key = (ffmpeg_path.to_path_buf(), target == VideoCompatibility::Hevc);
        if let Some(cached) = self.chosen.lock().unwrap().get(&key) {
            return *cached;
        }
        let available = compiled_encoders(ffmpeg_path, cancel).await;
        let mut chosen = None;
        for family in candidates {
            let Some(encoder) = family.encoder(target) else { continue };
            if !available.is_empty() && !available.iter().any(|e| e == encoder) {
                continue;
            }
            if test_encode(ffmpeg_path, *family, encoder, cancel).await {
                chosen = Some(*family);
                break;
            }
        }
        if !cancel.is_cancelled() {
            self.chosen.lock().unwrap().insert(key, chosen);
        }
        chosen
    }
}

async fn compiled_encoders(ffmpeg_path: &Path, cancel: &CancellationToken) -> Vec<String> {
    let mut request = ProcessRequest::new(ffmpeg_path, vec!["-hide_banner".into(), "-encoders".into()]);
    request.timeout = Some(Duration::from_secs(20));
    request.working_directory = Some(std::env::temp_dir());
    let Ok(output) = process::run_collect(&request, cancel).await else { return Vec::new() };
    String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| line.split_whitespace().nth(1).map(str::to_string)).collect()
}

/// Encodes a fraction of a second of generated video and discards it. Hardware encoders are compiled in
/// regardless of whether the matching graphics card is present, so this is the only reliable check.
async fn test_encode(ffmpeg_path: &Path, family: EncoderFamily, encoder: &str, cancel: &CancellationToken) -> bool {
    let arguments: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=640x360:r=30:d=0.5",
        "-frames:v",
        "10",
        "-pix_fmt",
        family.pixel_format(),
        "-c:v",
        encoder,
        "-b:v",
        "1000000",
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let mut request = ProcessRequest::new(ffmpeg_path, arguments);
    request.timeout = Some(Duration::from_secs(30));
    request.working_directory = Some(std::env::temp_dir());
    matches!(process::run_collect(&request, cancel).await, Ok(output) if output.exit.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ffmpeg::{AudioStream, VideoStream};

    fn probe(codec: &str, height: u32, audio: Option<&str>) -> MediaProbe {
        MediaProbe {
            video: Some(VideoStream {
                codec: codec.into(),
                width: None,
                height: Some(height),
                frame_rate: Some(30.0),
                bitrate: Some(10_000_000),
            }),
            audio: audio.map(|c| AudioStream { codec: c.into(), sample_rate: Some(48_000), channels: Some(2), bitrate: None }),
            duration: Some(10.0),
            ..Default::default()
        }
    }

    fn format(height: u32, codec: &str) -> MediaFormat {
        MediaFormat {
            id: format!("{height}{codec}"),
            height: Some(height),
            video_codec: Some(codec.into()),
            audio_codec: Some("none".into()),
            ..Default::default()
        }
    }

    #[test]
    fn only_formats_most_players_cant_open_are_converted() {
        for (codec, expected) in [("vp9", true), ("av1", true), ("h264", false), ("hevc", false), ("H264", false), ("mpeg4", true)] {
            assert_eq!(needs_conversion(&probe(codec, 1080, Some("aac"))), expected, "{codec}");
        }
        assert!(!needs_conversion(&MediaProbe::default()));
    }

    #[test]
    fn bitrate_follows_the_source_within_sensible_limits() {
        assert_eq!(target_bitrate(Some(2160), Some(30.0), Some(12_000_000), VideoCompatibility::H264), 30_000_000);
        assert_eq!(target_bitrate(Some(2160), Some(60.0), Some(40_000_000), VideoCompatibility::H264), 68_000_000);
        assert_eq!(target_bitrate(Some(1080), Some(30.0), Some(100_000), VideoCompatibility::H264), 3_200_000);
        assert_eq!(target_bitrate(Some(1080), None, None, VideoCompatibility::H264), 6_000_000);
        assert_eq!(target_bitrate(Some(2160), Some(30.0), Some(12_000_000), VideoCompatibility::Hevc), 18_000_000);
    }

    #[test]
    fn conversion_keeps_compatible_audio_and_stays_on_local_files() {
        let input = Path::new("/w/source.webm");
        let output = Path::new("/w/compatible.mp4");
        for family in [
            EncoderFamily::VideoToolbox,
            EncoderFamily::Nvenc,
            EncoderFamily::QuickSync,
            EncoderFamily::Amf,
            EncoderFamily::MediaFoundation,
        ] {
            let args: Vec<String> = arguments(input, output, &probe("vp9", 2160, Some("aac")), VideoCompatibility::H264, family)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].clone());
            assert_eq!(after("-c:v").as_deref(), family.encoder(VideoCompatibility::H264));
            assert_eq!(after("-c:a").as_deref(), Some("copy"));
            assert_eq!(after("-protocol_whitelist").as_deref(), Some("file,crypto,data"));
            assert_eq!(args.last().map(String::as_str), Some("file:/w/compatible.mp4"));
            assert!(args.contains(&"-n".to_string()));
            assert_eq!(after("-f").as_deref(), Some("mp4"));
        }
        let opus: Vec<String> =
            arguments(input, output, &probe("vp9", 1080, Some("opus")), VideoCompatibility::Hevc, EncoderFamily::VideoToolbox)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
        let after = |flag: &str| opus.iter().position(|a| a == flag).map(|i| opus[i + 1].clone());
        assert_eq!(after("-c:a").as_deref(), Some("aac"));
        assert_eq!(after("-tag:v").as_deref(), Some("hvc1"));
        assert!(arguments(input, output, &probe("vp9", 1080, None), VideoCompatibility::Original, EncoderFamily::Nvenc).is_empty());
    }

    #[test]
    fn predicts_conversion_from_what_a_source_offers_at_the_chosen_quality() {
        let formats = vec![
            format(2160, "vp09.00.50.08"),
            format(1440, "vp9"),
            format(1080, "avc1.640028"),
            format(1080, "vp9"),
            format(720, "avc1.4d401f"),
        ];
        assert!(expects_conversion(&formats, None));
        assert!(expects_conversion(&formats, Some(2160)));
        assert!(expects_conversion(&formats, Some(1440)));
        assert!(!expects_conversion(&formats, Some(1080)));
        assert!(!expects_conversion(&formats, Some(720)));
        // Asking for less than the source's smallest stream gets the smallest one.
        assert!(!expects_conversion(&formats, Some(360)));
        assert!(!expects_conversion(&[], Some(1080)));
    }

    #[test]
    fn every_platform_has_an_encoder_to_try() {
        assert!(!EncoderFamily::candidates().is_empty());
        assert_eq!(EncoderFamily::MediaFoundation.encoder(VideoCompatibility::Hevc), Some("hevc_mf"));
        assert_eq!(EncoderFamily::Nvenc.encoder(VideoCompatibility::Original), None);
    }
}
