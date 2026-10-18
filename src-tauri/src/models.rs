//! Output options and the other value types shared by the engine, the stores and the UI.
//!
//! Serialised names match the TypeScript definitions in `src/api/types.ts`.

use serde::{Deserialize, Serialize};

/// Audio container/codec combinations MediaForge can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Mp3,
    Wav,
    Aac,
    M4a,
    Flac,
    Aiff,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 6] = [Self::Mp3, Self::Wav, Self::Aac, Self::M4a, Self::Flac, Self::Aiff];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Aac => "aac",
            Self::M4a => "m4a",
            Self::Flac => "flac",
            Self::Aiff => "aiff",
        }
    }

    pub fn display_name(self) -> String {
        self.extension().to_uppercase()
    }

    /// Lossy formats take a bitrate; lossless ones take a bit depth.
    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Mp3 | Self::Aac | Self::M4a)
    }

    /// FFmpeg muxer name. Always passed explicitly so the temporary file name never decides the container.
    pub fn muxer(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Aac => "adts",
            Self::M4a => "ipod",
            Self::Flac => "flac",
            Self::Aiff => "aiff",
        }
    }

    /// The `codec_name` ffprobe reports for a stream that could be copied into this format untouched.
    pub fn copyable_codec(self) -> Option<&'static str> {
        match self {
            Self::Mp3 => Some("mp3"),
            Self::Aac | Self::M4a => Some("aac"),
            Self::Wav | Self::Flac | Self::Aiff => None,
        }
    }

    /// ADTS has no metadata container.
    pub fn supports_tags(self) -> bool {
        self != Self::Aac
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.extension() == text)
    }
}

macro_rules! numeric_option {
    ($name:ident { $($variant:ident = $value:expr),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "u32", into = "u32")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$(Self::$variant),+];
            pub fn value(self) -> u32 {
                match self { $(Self::$variant => $value),+ }
            }
            pub fn from_value(value: u32) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.value() == value)
            }
        }

        impl TryFrom<u32> for $name {
            type Error = String;
            fn try_from(value: u32) -> Result<Self, String> {
                Self::from_value(value).ok_or_else(|| format!("{} is not a valid {}", value, stringify!($name)))
            }
        }

        impl From<$name> for u32 {
            fn from(value: $name) -> u32 { value.value() }
        }
    };
}

numeric_option!(AudioBitrate { Kbps128 = 128, Kbps192 = 192, Kbps256 = 256, Kbps320 = 320 });
numeric_option!(SampleRate { Source = 0, Hz44100 = 44_100, Hz48000 = 48_000 });
numeric_option!(BitDepth { Pcm16 = 16, Pcm24 = 24 });
numeric_option!(VideoQuality { Best = 0, P2160 = 2160, P1440 = 1440, P1080 = 1080, P720 = 720, P480 = 480, P360 = 360 });
numeric_option!(RateLimitOption { Unlimited = 0, Mbps1 = 1, Mbps2 = 2, Mbps5 = 5, Mbps10 = 10, Mbps25 = 25 });
numeric_option!(SizeLimitOption { NoLimit = 0, Mb500 = 500, Gb1 = 1000, Gb2 = 2000, Gb5 = 5000, Gb10 = 10_000 });

impl AudioBitrate {
    pub fn display_name(self) -> String {
        format!("{} kbps", self.value())
    }

    pub fn bits_per_second(self) -> u64 {
        self.value() as u64 * 1000
    }
}

impl SampleRate {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Source => "Same as source",
            Self::Hz44100 => "44.1 kHz",
            Self::Hz48000 => "48 kHz",
        }
    }
}

impl BitDepth {
    pub fn display_name(self) -> String {
        format!("{}-bit", self.value())
    }
}

impl VideoQuality {
    pub fn short_name(self) -> String {
        if self == Self::Best { "Best".into() } else { format!("{}p", self.value()) }
    }

    /// Maximum frame height, or `None` for no cap.
    pub fn max_height(self) -> Option<u32> {
        if self == Self::Best { None } else { Some(self.value()) }
    }

    /// Qualities worth offering for a source whose tallest video stream is `height` pixels.
    pub fn options_for_source_height(height: Option<u32>) -> Vec<VideoQuality> {
        let Some(height) = height else { return vec![Self::Best] };
        let mut options = vec![Self::Best];
        options.extend(Self::ALL.iter().copied().filter(|q| q.max_height().is_some_and(|cap| cap <= height)));
        options
    }
}

impl RateLimitOption {
    pub fn bytes_per_second(self) -> Option<u64> {
        if self == Self::Unlimited { None } else { Some(self.value() as u64 * 1_000_000) }
    }
}

impl SizeLimitOption {
    pub fn bytes(self) -> Option<u64> {
        if self == Self::NoLimit { None } else { Some(self.value() as u64 * 1_000_000) }
    }
}

/// What to do with downloaded video in a format most players and editing apps can't open.
///
/// Above 1080p, large streaming sites usually offer only VP9 or AV1, which QuickTime, Quick Look, Final Cut
/// and Premiere can't read, so a 4K download is of little use until it has been converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCompatibility {
    H264,
    Hevc,
    Original,
}

impl VideoCompatibility {
    pub fn codec_name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::Original => "the original format",
        }
    }

    pub fn converts(self) -> bool {
        self != Self::Original
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOptions {
    pub format: AudioFormat,
    pub bitrate: AudioBitrate,
    pub sample_rate: SampleRate,
    pub bit_depth: BitDepth,
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self { format: AudioFormat::Mp3, bitrate: AudioBitrate::Kbps320, sample_rate: SampleRate::Source, bit_depth: BitDepth::Pcm16 }
    }
}

impl AudioOptions {
    pub fn summary(&self) -> String {
        let mut parts = vec![self.format.display_name()];
        parts.push(if self.format.is_lossy() { self.bitrate.display_name() } else { self.bit_depth.display_name() });
        if self.sample_rate != SampleRate::Source {
            parts.push(self.sample_rate.display_name().to_string());
        }
        parts.join(" · ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoOptions {
    pub quality: VideoQuality,
}

/// What a job should produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OutputSpec {
    Audio(AudioOptions),
    Video(VideoOptions),
}

impl OutputSpec {
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio(_))
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Audio(options) => options.format.extension(),
            Self::Video(_) => "mp4",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Audio(options) => options.summary(),
            Self::Video(options) => format!("MP4 · {}", options.quality.short_name()),
        }
    }

    pub fn badge(&self) -> String {
        match self {
            Self::Audio(options) => options.format.display_name(),
            Self::Video(_) => "MP4".into(),
        }
    }
}

/// Network and size ceilings applied to every download.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadLimits {
    /// Bytes per second, or `None` for unlimited.
    pub rate_limit: Option<u64>,
    /// Largest single file yt-dlp may fetch, or `None` for no cap.
    pub max_file_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadMode {
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    Downloads,
    Conversions,
    History,
    Settings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_spec_serialises_as_a_tagged_object() {
        let audio = OutputSpec::Audio(AudioOptions::default());
        let json = serde_json::to_value(audio).unwrap();
        assert_eq!(json, serde_json::json!({"type": "audio", "format": "mp3", "bitrate": 320, "sampleRate": 0, "bitDepth": 16}));
        let video: OutputSpec = serde_json::from_value(serde_json::json!({"type": "video", "quality": 2160})).unwrap();
        assert_eq!(video, OutputSpec::Video(VideoOptions { quality: VideoQuality::P2160 }));
        assert!(serde_json::from_value::<OutputSpec>(serde_json::json!({"type": "video", "quality": 999})).is_err());
    }

    #[test]
    fn summaries_and_badges() {
        let mut options = AudioOptions::default();
        assert_eq!(OutputSpec::Audio(options).summary(), "MP3 · 320 kbps");
        options.format = AudioFormat::Flac;
        options.bit_depth = BitDepth::Pcm24;
        options.sample_rate = SampleRate::Hz48000;
        assert_eq!(options.summary(), "FLAC · 24-bit · 48 kHz");
        assert_eq!(OutputSpec::Video(VideoOptions { quality: VideoQuality::P1080 }).summary(), "MP4 · 1080p");
        assert_eq!(OutputSpec::Video(VideoOptions { quality: VideoQuality::Best }).badge(), "MP4");
    }

    #[test]
    fn quality_options_follow_the_source() {
        assert_eq!(VideoQuality::options_for_source_height(None), vec![VideoQuality::Best]);
        assert_eq!(
            VideoQuality::options_for_source_height(Some(1080)),
            vec![VideoQuality::Best, VideoQuality::P1080, VideoQuality::P720, VideoQuality::P480, VideoQuality::P360]
        );
        assert_eq!(VideoQuality::options_for_source_height(Some(2160)).len(), 7);
    }

    #[test]
    fn limits_convert_to_bytes() {
        assert_eq!(RateLimitOption::Unlimited.bytes_per_second(), None);
        assert_eq!(RateLimitOption::Mbps5.bytes_per_second(), Some(5_000_000));
        assert_eq!(SizeLimitOption::Gb2.bytes(), Some(2_000_000_000));
    }
}
