//! Every failure the app can surface, phrased for people rather than for logs.

use serde::Serialize;

use crate::tools::Tool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaForgeError {
    ToolMissing(Tool),
    UnsupportedSource,
    DrmProtected,
    AuthenticationRequired,
    GeoRestricted,
    Unavailable,
    Network,
    RateLimited,
    AccessDenied,
    LiveStream,
    NoMatchingFormat,
    ExceedsSizeLimit,
    NoAudioStream,
    UnreadableFile(String),
    SourceFileMissing(String),
    ConversionFailed(String),
    DownloadFailed(String),
    DestinationUnavailable(String),
    InsufficientDiskSpace,
    TimedOut,
    InvalidMetadata,
    Unknown(String),
}

impl MediaForgeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ToolMissing(_) => "toolMissing",
            Self::UnsupportedSource => "unsupportedSource",
            Self::DrmProtected => "drmProtected",
            Self::AuthenticationRequired => "authenticationRequired",
            Self::GeoRestricted => "geoRestricted",
            Self::Unavailable => "unavailable",
            Self::Network => "network",
            Self::RateLimited => "rateLimited",
            Self::AccessDenied => "accessDenied",
            Self::LiveStream => "liveStream",
            Self::NoMatchingFormat => "noMatchingFormat",
            Self::ExceedsSizeLimit => "exceedsSizeLimit",
            Self::NoAudioStream => "noAudioStream",
            Self::UnreadableFile(_) => "unreadableFile",
            Self::SourceFileMissing(_) => "sourceFileMissing",
            Self::ConversionFailed(_) => "conversionFailed",
            Self::DownloadFailed(_) => "downloadFailed",
            Self::DestinationUnavailable(_) => "destinationUnavailable",
            Self::InsufficientDiskSpace => "insufficientDiskSpace",
            Self::TimedOut => "timedOut",
            Self::InvalidMetadata => "invalidMetadata",
            Self::Unknown(_) => "unknown",
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::ToolMissing(tool) => format!("{} not found", tool.display_name()),
            Self::UnsupportedSource => "Unsupported source".into(),
            Self::DrmProtected => "DRM-protected media".into(),
            Self::AuthenticationRequired => "Sign-in required".into(),
            Self::GeoRestricted => "Not available in your region".into(),
            Self::Unavailable => "Media unavailable".into(),
            Self::Network => "Network problem".into(),
            Self::RateLimited => "Too many requests".into(),
            Self::AccessDenied => "Access denied by source".into(),
            Self::LiveStream => "Live streams aren't supported".into(),
            Self::NoMatchingFormat => "No matching format".into(),
            Self::ExceedsSizeLimit => "Larger than your size limit".into(),
            Self::NoAudioStream => "No audio track".into(),
            Self::UnreadableFile(_) => "Unreadable file".into(),
            Self::SourceFileMissing(_) => "File not found".into(),
            Self::ConversionFailed(_) => "Conversion failed".into(),
            Self::DownloadFailed(_) => "Download failed".into(),
            Self::DestinationUnavailable(_) => "Can't write to destination".into(),
            Self::InsufficientDiskSpace => "Not enough disk space".into(),
            Self::TimedOut => "Timed out".into(),
            Self::InvalidMetadata => "Couldn't read media details".into(),
            Self::Unknown(_) => "Something went wrong".into(),
        }
    }

    pub fn message(&self) -> String {
        let or_default = |detail: &str, fallback: &str| {
            if detail.is_empty() { fallback.to_string() } else { detail.to_string() }
        };
        match self {
            Self::ToolMissing(tool) => tool.missing_advice(),
            Self::UnsupportedSource => {
                "This link isn't a source MediaForge can read. Check the address, or try the page that hosts the media directly.".into()
            }
            Self::DrmProtected => "This media is protected by DRM. MediaForge does not bypass copy protection.".into(),
            Self::AuthenticationRequired => {
                "The source requires an account or sign-in to access this media. MediaForge does not bypass access restrictions.".into()
            }
            Self::GeoRestricted => {
                "The source doesn't make this media available in your region. MediaForge does not circumvent regional restrictions.".into()
            }
            Self::Unavailable => "The media has been removed, made private, or the link is wrong.".into(),
            Self::Network => "The source couldn't be reached. Check your internet connection and try again.".into(),
            Self::RateLimited => "The source is limiting requests right now. Wait a few minutes before trying again.".into(),
            Self::AccessDenied => "The source refused the request. It may not permit downloads of this media.".into(),
            Self::LiveStream => "This is a live or upcoming broadcast. Try again once a recording is available.".into(),
            Self::NoMatchingFormat => {
                "The source doesn't offer this media in the format you asked for. Try a different format or quality.".into()
            }
            Self::ExceedsSizeLimit => "This download is larger than the maximum size set in Settings.".into(),
            Self::NoAudioStream => "This file doesn't contain an audio track to convert.".into(),
            Self::UnreadableFile(detail) => or_default(detail, "The file is damaged or isn't a media file MediaForge understands."),
            Self::SourceFileMissing(name) => format!("{name} has been moved or deleted."),
            Self::ConversionFailed(detail) => or_default(detail, "FFmpeg couldn't convert this media."),
            Self::DownloadFailed(detail) => or_default(detail, "The download didn't complete."),
            Self::DestinationUnavailable(detail) => detail.clone(),
            Self::InsufficientDiskSpace => "Free up some space on the destination disk and try again.".into(),
            Self::TimedOut => "The operation took too long and was stopped.".into(),
            Self::InvalidMetadata => "The source returned details MediaForge couldn't understand.".into(),
            Self::Unknown(detail) => or_default(detail, "An unexpected error occurred."),
        }
    }

    /// Whether running the same job again could plausibly succeed without the user changing anything.
    pub fn is_retryable(&self) -> bool {
        !matches!(
            self,
            Self::UnsupportedSource
                | Self::DrmProtected
                | Self::AuthenticationRequired
                | Self::GeoRestricted
                | Self::LiveStream
                | Self::NoAudioStream
                | Self::UnreadableFile(_)
        )
    }

    pub fn payload(&self) -> ErrorPayload {
        ErrorPayload { code: self.code(), title: self.title(), message: self.message(), retryable: self.is_retryable() }
    }
}

impl std::fmt::Display for MediaForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.title(), self.message())
    }
}

impl std::error::Error for MediaForgeError {}

/// How an error reaches the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub code: &'static str,
    pub title: String,
    pub message: String,
    pub retryable: bool,
}

impl Serialize for MediaForgeError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.payload().serialize(serializer)
    }
}

/// The two ways a piece of work can stop early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Cancelled,
    Error(MediaForgeError),
}

impl From<MediaForgeError> for Failure {
    fn from(error: MediaForgeError) -> Self {
        Self::Error(error)
    }
}

pub type JobResult<T> = Result<T, Failure>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_user_facing_text() {
        let all = [
            MediaForgeError::ToolMissing(Tool::YtDlp),
            MediaForgeError::UnsupportedSource,
            MediaForgeError::DrmProtected,
            MediaForgeError::AuthenticationRequired,
            MediaForgeError::GeoRestricted,
            MediaForgeError::Unavailable,
            MediaForgeError::Network,
            MediaForgeError::RateLimited,
            MediaForgeError::AccessDenied,
            MediaForgeError::LiveStream,
            MediaForgeError::NoMatchingFormat,
            MediaForgeError::ExceedsSizeLimit,
            MediaForgeError::NoAudioStream,
            MediaForgeError::UnreadableFile(String::new()),
            MediaForgeError::SourceFileMissing("x.wav".into()),
            MediaForgeError::ConversionFailed(String::new()),
            MediaForgeError::DownloadFailed(String::new()),
            MediaForgeError::DestinationUnavailable("Nope.".into()),
            MediaForgeError::InsufficientDiskSpace,
            MediaForgeError::TimedOut,
            MediaForgeError::InvalidMetadata,
            MediaForgeError::Unknown(String::new()),
        ];
        for error in all {
            assert!(!error.title().is_empty());
            assert!(!error.message().is_empty(), "{error:?}");
            assert!(!error.message().to_lowercase().contains("homebrew") || cfg!(target_os = "macos"));
        }
    }

    #[test]
    fn restrictions_are_not_retryable() {
        assert!(!MediaForgeError::DrmProtected.is_retryable());
        assert!(!MediaForgeError::AuthenticationRequired.is_retryable());
        assert!(MediaForgeError::Network.is_retryable());
        let json = serde_json::to_value(MediaForgeError::GeoRestricted).unwrap();
        assert_eq!(json["code"], "geoRestricted");
        assert_eq!(json["retryable"], false);
    }
}
