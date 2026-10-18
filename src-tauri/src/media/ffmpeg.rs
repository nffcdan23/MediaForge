//! Drives ffprobe and ffmpeg for inspection and conversion.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::{Failure, MediaForgeError};
use crate::models::{AudioFormat, AudioOptions, BitDepth, SampleRate};
use crate::process::{self, ProcessError, ProcessEvent, ProcessExit, ProcessRequest};
use crate::tools::Tool;

/// Local files only. Without this a crafted playlist or concat script could make FFmpeg open network URLs.
pub const PROTOCOL_WHITELIST: &str = "file,crypto,data";

/// Prefixing with `file:` stops FFmpeg treating odd file names as options or as other protocols.
pub fn file_argument(path: &Path) -> OsString {
    let mut argument = OsString::from("file:");
    argument.push(path.as_os_str());
    argument
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStream {
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    /// Bits per second, when the container records it.
    pub bitrate: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<f64>,
    pub bitrate: Option<u64>,
}

/// What ffprobe found in a file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaProbe {
    pub duration: Option<f64>,
    pub format_name: Option<String>,
    pub audio: Option<AudioStream>,
    /// The first real video stream. Cover art attached to an audio file doesn't count.
    pub video: Option<VideoStream>,
    /// True when the file carries a picture stream such as embedded cover art.
    pub has_cover_art: bool,
    pub title: Option<String>,
    pub artist: Option<String>,
}

impl MediaProbe {
    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }

    /// ffprobe writes frame rates as fractions such as "30000/1001".
    pub fn parse_rate(text: Option<&str>) -> Option<f64> {
        let text = text?;
        let mut parts = text.split('/');
        let numerator: f64 = parts.next()?.parse().ok().filter(|n: &f64| *n > 0.0)?;
        let denominator: f64 = match parts.next() {
            Some(d) => d.parse().unwrap_or(0.0),
            None => 1.0,
        };
        if denominator <= 0.0 {
            return None;
        }
        let rate = numerator / denominator;
        (rate.is_finite() && rate < 1000.0).then_some(rate)
    }

    pub fn decode(data: &[u8]) -> Result<Self, MediaForgeError> {
        let raw: Value = serde_json::from_slice(data).map_err(|_| MediaForgeError::UnreadableFile(String::new()))?;
        let streams: Vec<&Value> = raw.get("streams").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        let string = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        let number = |value: &Value, key: &str| match value.get(key) {
            Some(Value::String(text)) => text.parse::<f64>().ok(),
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        };
        let is_picture = |stream: &Value| stream.get("disposition").and_then(|d| d.get("attached_pic")).and_then(Value::as_i64) == Some(1);
        let kind = |stream: &Value| string(stream, "codec_type");
        let audio = streams.iter().find(|s| kind(s).as_deref() == Some("audio"));
        let video = streams.iter().find(|s| kind(s).as_deref() == Some("video") && !is_picture(s));
        let cover = streams.iter().any(|s| kind(s).as_deref() == Some("video") && is_picture(s));
        let format = raw.get("format");
        let duration = [format.and_then(|f| number(f, "duration")), audio.and_then(|a| number(a, "duration"))]
            .into_iter()
            .flatten()
            .find(|d| d.is_finite() && *d > 0.0);
        let tag = |key: &str| {
            let tags = format?.get("tags")?.as_object()?;
            tags.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).and_then(|(_, v)| v.as_str()).map(str::to_string)
        };
        let positive = |value: Option<f64>| value.filter(|v| v.is_finite() && *v > 0.0);

        Ok(MediaProbe {
            duration,
            format_name: format.and_then(|f| string(f, "format_name")),
            audio: audio.and_then(|stream| {
                Some(AudioStream {
                    codec: string(stream, "codec_name")?,
                    sample_rate: positive(number(stream, "sample_rate")).map(|v| v as u32),
                    channels: positive(number(stream, "channels")).map(|v| v as u32),
                    bitrate: positive(number(stream, "bit_rate")).map(|v| v as u64),
                })
            }),
            video: video.and_then(|stream| {
                Some(VideoStream {
                    codec: string(stream, "codec_name")?,
                    width: positive(number(stream, "width")).map(|v| v as u32),
                    height: positive(number(stream, "height")).map(|v| v as u32),
                    frame_rate: Self::parse_rate(stream.get("avg_frame_rate").and_then(Value::as_str))
                        .or_else(|| Self::parse_rate(stream.get("r_frame_rate").and_then(Value::as_str))),
                    bitrate: positive(number(stream, "bit_rate")).map(|v| v as u64),
                })
            }),
            has_cover_art: cover,
            title: tag("title"),
            artist: tag("artist"),
        })
    }
}

/// Tags written into converted files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub comment: Option<String>,
}

impl MediaTags {
    pub fn arguments(&self) -> Vec<OsString> {
        let mut arguments = Vec::new();
        for (key, value) in [("title", &self.title), ("artist", &self.artist), ("comment", &self.comment)] {
            let Some(value) = value.as_deref().map(clean_tag).filter(|v| !v.is_empty()) else { continue };
            arguments.push("-metadata".into());
            arguments.push(format!("{key}={value}").into());
        }
        arguments
    }
}

fn clean_tag(value: &str) -> String {
    value.chars().filter(|c| (*c as u32) >= 0x20 && *c != '\u{7F}').take(500).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ConversionProgress {
    pub fraction: Option<f64>,
    /// Multiple of real time.
    pub speed: Option<f64>,
    pub seconds_remaining: Option<f64>,
}

/// Interprets FFmpeg's `-progress` key=value stream.
#[derive(Debug, Clone)]
pub struct ProgressParser {
    duration: Option<f64>,
    position: Option<f64>,
    speed: Option<f64>,
}

impl ProgressParser {
    pub fn new(duration: Option<f64>) -> Self {
        Self { duration: duration.filter(|d| *d > 0.0), position: None, speed: None }
    }

    /// Feed one line. Returns a snapshot each time FFmpeg closes a progress block.
    pub fn consume(&mut self, line: &str) -> Option<ConversionProgress> {
        let (key, value) = line.split_once('=')?;
        let (key, value) = (key.trim(), value.trim());
        match key {
            "out_time_us" => {
                if let Ok(microseconds) = value.parse::<f64>()
                    && microseconds >= 0.0
                {
                    self.position = Some(microseconds / 1_000_000.0);
                }
            }
            "out_time" => {
                if self.position.is_none() {
                    self.position = parse_clock(value);
                }
            }
            "speed" => {
                let number = value.strip_suffix('x').unwrap_or(value);
                if let Ok(parsed) = number.trim().parse::<f64>()
                    && parsed.is_finite()
                    && parsed > 0.0
                {
                    self.speed = Some(parsed);
                }
            }
            "progress" => {
                let mut progress = ConversionProgress { speed: self.speed, ..Default::default() };
                if value == "end" {
                    progress.fraction = Some(1.0);
                    progress.seconds_remaining = Some(0.0);
                } else if let (Some(duration), Some(position)) = (self.duration, self.position) {
                    progress.fraction = Some((position / duration).clamp(0.0, 1.0));
                    progress.seconds_remaining = self.speed.map(|speed| (duration - position).max(0.0) / speed);
                }
                return Some(progress);
            }
            _ => {}
        }
        None
    }
}

/// `HH:MM:SS.ffffff`
pub fn parse_clock(text: &str) -> Option<f64> {
    let parts: Vec<f64> = text.split(':').map(|p| p.parse::<f64>().ok()).collect::<Option<_>>()?;
    if parts.len() != 3 || parts.iter().any(|p| *p < 0.0) {
        return None;
    }
    Some(parts[0] * 3600.0 + parts[1] * 60.0 + parts[2])
}

pub fn probe_arguments(file: &Path) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "-v",
        "error",
        "-hide_banner",
        "-protocol_whitelist",
        PROTOCOL_WHITELIST,
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-i",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    arguments.push(file_argument(file));
    arguments
}

/// A stream can be copied bit-for-bit when it's already in the target codec, the sample rate isn't changing,
/// and re-encoding at the chosen bitrate could only lose quality.
pub fn can_stream_copy(probe: &MediaProbe, options: &AudioOptions) -> bool {
    let (Some(audio), Some(codec)) = (&probe.audio, options.format.copyable_codec()) else { return false };
    if audio.codec != codec {
        return false;
    }
    if options.sample_rate != SampleRate::Source && Some(options.sample_rate.value()) != audio.sample_rate {
        return false;
    }
    match audio.bitrate {
        Some(bitrate) if bitrate > 0 => bitrate as f64 <= options.bitrate.bits_per_second() as f64 * 1.05,
        _ => false,
    }
}

pub fn codec_arguments(options: &AudioOptions, probe: Option<&MediaProbe>) -> Vec<String> {
    if probe.is_some_and(|p| can_stream_copy(p, options)) {
        let mut arguments = vec!["-c:a".to_string(), "copy".into()];
        if options.format == AudioFormat::M4a {
            arguments.extend(["-movflags".into(), "+faststart".into()]);
        }
        return arguments;
    }
    let bitrate = format!("{}k", options.bitrate.value());
    let twenty_four = options.bit_depth == BitDepth::Pcm24;
    let mut arguments: Vec<String> = match options.format {
        AudioFormat::Mp3 => vec!["-c:a".into(), "libmp3lame".into(), "-b:a".into(), bitrate, "-id3v2_version".into(), "3".into()],
        AudioFormat::Aac => vec!["-c:a".into(), "aac".into(), "-b:a".into(), bitrate],
        AudioFormat::M4a => vec!["-c:a".into(), "aac".into(), "-b:a".into(), bitrate, "-movflags".into(), "+faststart".into()],
        AudioFormat::Wav => vec!["-c:a".into(), if twenty_four { "pcm_s24le" } else { "pcm_s16le" }.into()],
        AudioFormat::Aiff => {
            vec!["-c:a".into(), if twenty_four { "pcm_s24be" } else { "pcm_s16be" }.into(), "-write_id3v2".into(), "1".into()]
        }
        AudioFormat::Flac => {
            let mut flac = vec!["-c:a".to_string(), "flac".into(), "-compression_level".into(), "8".into()];
            if twenty_four {
                flac.extend(["-sample_fmt".into(), "s32".into(), "-bits_per_raw_sample".into(), "24".into()]);
            } else {
                flac.extend(["-sample_fmt".into(), "s16".into()]);
            }
            flac
        }
    };
    if options.sample_rate != SampleRate::Source {
        arguments.extend(["-ar".into(), options.sample_rate.value().to_string()]);
    }
    arguments
}

/// The fixed options at the start of every conversion: quiet, never overwrite, progress on stdout.
fn conversion_preamble(input: &Path) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-loglevel",
        "error",
        "-n", // never overwrite; the output path is always fresh
        "-progress",
        "pipe:1",
        "-stats_period",
        "0.25",
        "-protocol_whitelist",
        PROTOCOL_WHITELIST,
        "-i",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    arguments.push(file_argument(input));
    arguments
}

pub fn conversion_arguments(
    input: &Path,
    output: &Path,
    options: &AudioOptions,
    probe: Option<&MediaProbe>,
    tags: &MediaTags,
) -> Vec<OsString> {
    let mut arguments = conversion_preamble(input);
    // First audio stream only; no video, subtitles, data or chapters.
    arguments.extend(["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_chapters", "-1"].iter().map(OsString::from));
    arguments.extend(codec_arguments(options, probe).into_iter().map(OsString::from));
    if options.format.supports_tags() {
        arguments.extend(tags.arguments());
    }
    arguments.push("-f".into());
    arguments.push(options.format.muxer().into());
    arguments.push(file_argument(output));
    arguments
}

pub(crate) fn video_preamble(input: &Path) -> Vec<OsString> {
    conversion_preamble(input)
}

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Ffmpeg {
    pub async fn probe(&self, file: &Path, cancel: &CancellationToken) -> Result<MediaProbe, Failure> {
        if std::fs::File::open(file).is_err() {
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(MediaForgeError::SourceFileMissing(name).into());
        }
        let mut request = ProcessRequest::new(&self.ffprobe, probe_arguments(file));
        request.working_directory = Some(std::env::temp_dir());
        request.timeout = Some(Duration::from_secs(60));
        request.max_output_bytes = 16 * 1024 * 1024;
        let output = process::run_collect(&request, cancel).await.map_err(|e| translate(e, Tool::Ffprobe))?;
        if !output.exit.success() {
            return Err(MediaForgeError::UnreadableFile(String::new()).into());
        }
        Ok(MediaProbe::decode(&output.stdout)?)
    }

    /// Runs FFmpeg with `arguments`, streaming its output: progress on stdout, diagnostics on stderr.
    pub async fn run(
        &self,
        arguments: Vec<OsString>,
        working_directory: &Path,
        timeout: Duration,
        cancel: &CancellationToken,
        on_event: impl FnMut(ProcessEvent),
    ) -> Result<ProcessExit, Failure> {
        let mut request = ProcessRequest::new(&self.ffmpeg, arguments);
        request.working_directory = Some(working_directory.to_path_buf());
        request.timeout = Some(timeout);
        // FFmpeg reports four times a second while it's working.
        request.idle_timeout = Some(Duration::from_secs(300));
        request.max_output_bytes = 64 * 1024 * 1024;
        process::run_streaming(&request, cancel, on_event).await.map_err(|e| translate(e, Tool::Ffmpeg))
    }
}

pub fn translate(error: ProcessError, tool: Tool) -> Failure {
    match error {
        ProcessError::ExecutableNotFound(_) => MediaForgeError::ToolMissing(tool).into(),
        ProcessError::LaunchFailed(detail) => {
            MediaForgeError::ConversionFailed(format!("{} couldn't be started. {detail}", tool.display_name())).into()
        }
        ProcessError::TimedOut(_) | ProcessError::Stalled(_) => MediaForgeError::TimedOut.into(),
        ProcessError::OutputLimitExceeded(_) => {
            MediaForgeError::ConversionFailed(format!("{} produced an unexpected amount of output.", tool.display_name())).into()
        }
        ProcessError::Cancelled => Failure::Cancelled,
    }
}

const VIDEO_ENCODER_UNAVAILABLE: &str = "The video encoder isn't available with this copy of FFmpeg or this graphics hardware. Use the FFmpeg included with MediaForge, or keep the original format in Settings ▸ Video.";

/// As `summarize`, for a video re-encode: failures to open a hardware encoder get their own explanation.
pub fn summarize_video(lines: &[String]) -> String {
    let text = lines.join("\n").to_lowercase();
    let encoder_trouble = ["videotoolbox", "nvenc", "_qsv", "_amf", "h264_mf", "hevc_mf", "error while opening encoder", "unknown encoder"]
        .iter()
        .any(|needle| text.contains(needle));
    if encoder_trouble && !text.contains("no space left") {
        return VIDEO_ENCODER_UNAVAILABLE.into();
    }
    summarize(lines)
}

/// Turns FFmpeg's stderr into one sentence worth showing.
pub fn summarize(lines: &[String]) -> String {
    let text = lines.join("\n").to_lowercase();
    if text.contains("no space left") || text.contains("not enough space") {
        return "The disk is full.".into();
    }
    if text.contains("permission denied") || text.contains("operation not permitted") {
        return "MediaForge doesn't have permission to read or write this file.".into();
    }
    if text.contains("invalid data found") || text.contains("moov atom not found") {
        return "The file is damaged or isn't a media format FFmpeg understands.".into();
    }
    if text.contains("no such file") {
        return "The source file could not be found.".into();
    }
    if text.contains("matches no streams") {
        return "This file doesn't contain an audio track to convert.".into();
    }
    if text.contains("av1") && (text.contains("hardware accelerated") || text.contains("decoder")) {
        return "This video is only available in AV1, which can't be converted here. Choose a lower quality, or keep the original format in Settings ▸ Video.".into();
    }
    if text.contains("videotoolbox") || text.contains("unknown encoder") {
        return VIDEO_ENCODER_UNAVAILABLE.into();
    }
    let Some(line) = lines.iter().rev().find(|l| !l.trim().is_empty()) else { return String::new() };
    let mut line = line.as_str();
    // Drop the "[component @ 0x…]" prefix.
    if line.starts_with('[')
        && let Some(close) = line.find(']')
    {
        line = &line[close + 1..];
    }
    let line = line.trim();
    if line.chars().count() > 300 { line.chars().take(300).collect::<String>() + "…" } else { line.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AudioBitrate;

    fn text(arguments: &[OsString]) -> Vec<String> {
        arguments.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn value_after(arguments: &[String], flag: &str) -> Option<String> {
        arguments.iter().position(|a| a == flag).and_then(|i| arguments.get(i + 1)).cloned()
    }

    fn options(format: AudioFormat) -> AudioOptions {
        AudioOptions { format, ..AudioOptions::default() }
    }

    fn probe(codec: &str, sample_rate: u32, bitrate: Option<u64>) -> MediaProbe {
        MediaProbe {
            audio: Some(AudioStream { codec: codec.into(), sample_rate: Some(sample_rate), channels: Some(2), bitrate }),
            duration: Some(10.0),
            ..Default::default()
        }
    }

    #[test]
    fn paths_use_an_explicit_file_protocol_and_a_local_only_whitelist() {
        let input = Path::new("/tmp/-rf input.wav");
        let output = Path::new("/tmp/out.mp3");
        let arguments = text(&conversion_arguments(input, output, &options(AudioFormat::Mp3), None, &MediaTags::default()));
        assert_eq!(value_after(&arguments, "-i").as_deref(), Some("file:/tmp/-rf input.wav"));
        assert_eq!(arguments.last().map(String::as_str), Some("file:/tmp/out.mp3"));
        assert_eq!(value_after(&arguments, "-protocol_whitelist").as_deref(), Some("file,crypto,data"));
        assert!(arguments.contains(&"-n".to_string()));
        assert!(arguments.contains(&"-nostdin".to_string()));
        assert_eq!(text(&probe_arguments(input)).last().map(String::as_str), Some("file:/tmp/-rf input.wav"));
    }

    #[test]
    fn each_format_gets_the_right_encoder_and_container() {
        for (format, codec, muxer) in [
            (AudioFormat::Mp3, "libmp3lame", "mp3"),
            (AudioFormat::Wav, "pcm_s16le", "wav"),
            (AudioFormat::Aac, "aac", "adts"),
            (AudioFormat::M4a, "aac", "ipod"),
            (AudioFormat::Flac, "flac", "flac"),
            (AudioFormat::Aiff, "pcm_s16be", "aiff"),
        ] {
            let arguments =
                text(&conversion_arguments(Path::new("/a.wav"), Path::new("/b"), &options(format), None, &MediaTags::default()));
            assert_eq!(value_after(&arguments, "-c:a").as_deref(), Some(codec), "{format:?}");
            assert_eq!(value_after(&arguments, "-f").as_deref(), Some(muxer), "{format:?}");
        }
    }

    #[test]
    fn mp3_bitrates_map_straight_through() {
        for bitrate in AudioBitrate::ALL {
            let mut audio = options(AudioFormat::Mp3);
            audio.bitrate = *bitrate;
            let arguments = codec_arguments(&audio, None);
            assert_eq!(value_after(&arguments, "-b:a"), Some(format!("{}k", bitrate.value())));
        }
    }

    #[test]
    fn bit_depth_and_sample_rate_apply_to_lossless_formats() {
        let mut flac = options(AudioFormat::Flac);
        flac.bit_depth = BitDepth::Pcm24;
        flac.sample_rate = SampleRate::Hz48000;
        let arguments = codec_arguments(&flac, None);
        assert_eq!(value_after(&arguments, "-sample_fmt").as_deref(), Some("s32"));
        assert_eq!(value_after(&arguments, "-bits_per_raw_sample").as_deref(), Some("24"));
        assert_eq!(value_after(&arguments, "-ar").as_deref(), Some("48000"));
        let mut wav = options(AudioFormat::Wav);
        wav.bit_depth = BitDepth::Pcm24;
        assert_eq!(value_after(&codec_arguments(&wav, None), "-c:a").as_deref(), Some("pcm_s24le"));
    }

    #[test]
    fn copies_the_stream_only_when_re_encoding_could_not_help() {
        let mut m4a = options(AudioFormat::M4a);
        m4a.bitrate = AudioBitrate::Kbps256;
        assert!(can_stream_copy(&probe("aac", 44_100, Some(128_000)), &m4a));
        assert_eq!(codec_arguments(&m4a, Some(&probe("aac", 44_100, Some(128_000))))[..2], ["-c:a", "copy"]);
        // Higher than asked for: re-encode down.
        assert!(!can_stream_copy(&probe("aac", 44_100, Some(320_000)), &m4a));
        // Unknown bitrate, other codec, or a sample-rate change: re-encode.
        assert!(!can_stream_copy(&probe("aac", 44_100, None), &m4a));
        assert!(!can_stream_copy(&probe("opus", 48_000, Some(128_000)), &m4a));
        m4a.sample_rate = SampleRate::Hz48000;
        assert!(!can_stream_copy(&probe("aac", 44_100, Some(128_000)), &m4a));
        // Lossless outputs are never copies.
        assert!(!can_stream_copy(&probe("flac", 44_100, Some(800_000)), &options(AudioFormat::Flac)));
    }

    #[test]
    fn tags_are_single_arguments_stripped_of_control_characters_and_skipped_for_adts() {
        let tags = MediaTags { title: Some("Hello\nWorld\u{7}".into()), artist: Some("-x".into()), comment: None };
        assert_eq!(text(&tags.arguments()), ["-metadata", "title=HelloWorld", "-metadata", "artist=-x"]);
        let adts = text(&conversion_arguments(Path::new("/a"), Path::new("/b"), &options(AudioFormat::Aac), None, &tags));
        assert!(!adts.contains(&"-metadata".to_string()));
    }

    #[test]
    fn turns_progress_blocks_into_fractions_speed_and_time_remaining() {
        let mut parser = ProgressParser::new(Some(100.0));
        assert_eq!(parser.consume("out_time_us=25000000"), None);
        assert_eq!(parser.consume("speed=5x"), None);
        let progress = parser.consume("progress=continue").unwrap();
        assert_eq!(progress.fraction, Some(0.25));
        assert_eq!(progress.speed, Some(5.0));
        assert_eq!(progress.seconds_remaining, Some(15.0));
        let end = parser.consume("progress=end").unwrap();
        assert_eq!(end.fraction, Some(1.0));
        assert_eq!(end.seconds_remaining, Some(0.0));
    }

    #[test]
    fn handles_na_values_and_unknown_durations() {
        let mut parser = ProgressParser::new(None);
        parser.consume("out_time_us=N/A");
        parser.consume("speed=N/A");
        let progress = parser.consume("progress=continue").unwrap();
        assert_eq!(progress.fraction, None);
        assert_eq!(progress.speed, None);
        let mut clock = ProgressParser::new(Some(60.0));
        clock.consume("out_time=00:00:30.000000");
        assert_eq!(clock.consume("progress=continue").unwrap().fraction, Some(0.5));
        assert_eq!(parse_clock("01:02:03.5"), Some(3723.5));
        assert_eq!(parse_clock("bad"), None);
    }

    #[test]
    fn reads_streams_duration_and_tags_and_ignores_cover_art() {
        let json = br#"{"streams": [
            {"codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1}},
            {"codec_type": "audio", "codec_name": "mp3", "sample_rate": "44100", "channels": 2, "bit_rate": "320000"}
        ], "format": {"duration": "187.5", "format_name": "mp3", "tags": {"TITLE": "Theme", "artist": "C2"}}}"#;
        let probe = MediaProbe::decode(json).unwrap();
        assert_eq!(probe.duration, Some(187.5));
        assert!(!probe.has_video());
        assert!(probe.has_cover_art);
        let audio = probe.audio.unwrap();
        assert_eq!((audio.codec.as_str(), audio.sample_rate, audio.bitrate), ("mp3", Some(44_100), Some(320_000)));
        assert_eq!(probe.title.as_deref(), Some("Theme"));
        assert_eq!(probe.artist.as_deref(), Some("C2"));
    }

    #[test]
    fn detects_real_video_and_missing_audio() {
        let json = br#"{"streams": [{"codec_type": "video", "codec_name": "vp9", "width": 3840, "height": 2160, "avg_frame_rate": "30000/1001"}], "format": {"duration": "10"}}"#;
        let probe = MediaProbe::decode(json).unwrap();
        let video = probe.video.clone().unwrap();
        assert_eq!((video.codec.as_str(), video.height), ("vp9", Some(2160)));
        assert!((video.frame_rate.unwrap() - 29.97).abs() < 0.01);
        assert!(probe.audio.is_none());
        assert_eq!(MediaProbe::parse_rate(Some("0/0")), None);
        assert_eq!(MediaProbe::parse_rate(Some("25")), Some(25.0));
        assert!(MediaProbe::decode(b"garbage").is_err());
    }

    #[test]
    fn summarises_common_failures() {
        assert_eq!(
            summarize(&["x.wav: Invalid data found when processing input".into()]),
            "The file is damaged or isn't a media format FFmpeg understands."
        );
        assert_eq!(summarize(&["[aost#0:0 @ 0x1] Error: something odd".into()]), "Error: something odd");
        assert!(summarize(&["Unknown encoder 'h264_nvenc'".into()]).contains("video encoder"));
    }
}
