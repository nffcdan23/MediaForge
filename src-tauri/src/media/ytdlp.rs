//! Drives yt-dlp. Argument construction is kept in pure functions so it can be audited and tested.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::error::{Failure, MediaForgeError};
use crate::media::metadata::{MAX_JSON_BYTES, MediaMetadata};
use crate::models::{DownloadLimits, OutputSpec, VideoCompatibility};
use crate::process::{self, ProcessError, ProcessEvent, ProcessExit, ProcessRequest};
use crate::support::url_validator::{MAX_BATCH_SIZE, ValidatedUrl};
use crate::tools::Tool;

/// Base name of everything a download writes into its working directory.
pub const OUTPUT_STEM: &str = "source";
pub const PROGRESS_PREFIX: &str = "MFP|";

pub fn info_json_name() -> String {
    format!("{OUTPUT_STEM}.info.json")
}

pub fn progress_template() -> String {
    let fields = [
        "%(progress.status)s",
        "%(progress.downloaded_bytes)s",
        "%(progress.total_bytes)s",
        "%(progress.total_bytes_estimate)s",
        "%(progress.speed)s",
        "%(progress.eta)s",
    ];
    format!("download:{PROGRESS_PREFIX}{}", fields.join("|"))
}

/// Best video plus best audio, else the best combined file that isn't known to be audio-only.
pub const VIDEO_FORMAT_SELECTOR: &str = "bv*+ba/b[vcodec!=?none]";

/// As above, but passing over AV1 while anything else is on offer. AV1 can't be played by most players and
/// editors, and is the slowest format to convert, whereas VP9 at the same resolution converts readily.
pub fn convertible_video_format_selector() -> String {
    format!("bv*[vcodec!^=?av01]+ba/{VIDEO_FORMAT_SELECTOR}")
}

/// Options applied to every invocation.
///
/// These pin yt-dlp to predictable, restriction-respecting behaviour: no user configuration or plugins (either
/// could inject `--exec`), no cookies or stored credentials (so sign-in walls stay walls), and no forged
/// `X-Forwarded-For` header (so regional restrictions are honoured). TLS verification is left on.
pub const BASE_ARGUMENTS: &[&str] = &[
    "--ignore-config",
    "--no-plugin-dirs",
    "--color",
    "never",
    "--no-cookies",
    "--no-cookies-from-browser",
    "--xff",
    "never",
    "--socket-timeout",
    "30",
];

fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

pub fn metadata_arguments(url: &ValidatedUrl) -> Vec<OsString> {
    let mut arguments = strings(BASE_ARGUMENTS);
    arguments.extend(strings(&["--dump-single-json", "--no-warnings", "--no-playlist", "--flat-playlist", "--playlist-items"]));
    arguments.push(format!("1:{MAX_BATCH_SIZE}").into());
    arguments.push("--".into());
    arguments.push(url.as_str().into());
    arguments
}

pub fn download_arguments(
    url: &ValidatedUrl,
    output: &OutputSpec,
    limits: DownloadLimits,
    embed_metadata: bool,
    video_compatibility: VideoCompatibility,
    ffmpeg_directory: Option<&Path>,
) -> Vec<OsString> {
    let mut arguments = strings(BASE_ARGUMENTS);
    arguments.extend(strings(&[
        "--no-playlist",
        "--playlist-items",
        "1",
        // A live broadcast has no end, so recording one would never finish.
        "--match-filters",
        "!is_live",
        "--newline",
        "--progress",
        "--progress-delta",
        "0.2",
        "--progress-template",
    ]));
    arguments.push(progress_template().into());
    arguments.extend(strings(&[
        "--write-info-json",
        "--no-mtime",
        "--retries",
        "5",
        "--fragment-retries",
        "5",
        "--abort-on-unavailable-fragments",
        "--concurrent-fragments",
        "4",
        // Relative to the job's private working directory; the remote title never reaches the filesystem.
        "--output",
    ]));
    arguments.push(format!("{OUTPUT_STEM}.%(ext)s").into());

    match output {
        OutputSpec::Audio(_) => {
            // Fetch the best audio the source has. MediaForge converts it itself so it can report progress.
            arguments.extend(strings(&["--format", "bestaudio/best"]));
        }
        // Images are fetched by MediaForge itself (media::image_link); this is only reached if one is passed
        // here anyway.
        OutputSpec::Image(_) => arguments.extend(strings(&["--format", "best"])),
        OutputSpec::Video(options) => {
            // Resolution first, then prefer H.264 + AAC so the MP4 opens in every editor.
            let resolution = options.quality.max_height().map(|h| format!("res:{h}")).unwrap_or_else(|| "res".into());
            let selector = if video_compatibility == VideoCompatibility::Original {
                VIDEO_FORMAT_SELECTOR.to_string()
            } else {
                convertible_video_format_selector()
            };
            // The filter stops yt-dlp quietly substituting an audio-only file when there's no video.
            arguments.push("--format".into());
            arguments.push(selector.into());
            arguments.push("--format-sort".into());
            arguments.push(format!("{resolution},fps,vcodec:h264,acodec:aac").into());
            arguments.extend(strings(&["--merge-output-format", "mp4", "--remux-video", "mp4"]));
            if embed_metadata {
                arguments.push("--embed-metadata".into());
            }
        }
    }

    if let Some(rate) = limits.rate_limit.filter(|r| *r > 0) {
        arguments.push("--limit-rate".into());
        arguments.push(rate.to_string().into());
    }
    if let Some(size) = limits.max_file_size.filter(|s| *s > 0) {
        arguments.push("--max-filesize".into());
        arguments.push(size.to_string().into());
    }
    if let Some(directory) = ffmpeg_directory {
        arguments.push("--ffmpeg-location".into());
        arguments.push(directory.as_os_str().to_owned());
    }
    // Everything after "--" is positional, so a link can never be read as an option.
    arguments.push("--".into());
    arguments.push(url.as_str().into());
    arguments
}

/// The tool plus everything it needs to find on its search path.
#[derive(Debug, Clone)]
pub struct YtDlp {
    pub executable: PathBuf,
    /// Directory containing ffmpeg and ffprobe, passed so yt-dlp never searches for them itself.
    pub ffmpeg_directory: Option<PathBuf>,
    /// Folders of programs yt-dlp may start itself (its JavaScript runtime), searched before system ones.
    pub support_directories: Vec<PathBuf>,
}

impl YtDlp {
    fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut prepend: Vec<&Path> = Vec::new();
        if let Some(parent) = self.executable.parent() {
            prepend.push(parent);
        }
        if let Some(directory) = &self.ffmpeg_directory {
            prepend.push(directory);
        }
        prepend.extend(self.support_directories.iter().map(PathBuf::as_path));
        process::env::sanitized(&prepend)
    }

    /// Resolves a link to its title, duration, artwork and available formats without downloading anything.
    pub async fn fetch_metadata(&self, url: &ValidatedUrl, cancel: &CancellationToken) -> Result<MediaMetadata, Failure> {
        let mut request = ProcessRequest::new(&self.executable, metadata_arguments(url));
        request.env = self.environment();
        request.working_directory = Some(std::env::temp_dir());
        request.timeout = Some(Duration::from_secs(120));
        request.max_output_bytes = MAX_JSON_BYTES;
        let output = process::run_collect(&request, cancel).await.map_err(translate)?;
        if !output.exit.success() {
            let lines: Vec<String> = output.stderr.lines().map(str::to_string).collect();
            return Err(classify(&lines).into());
        }
        let metadata = MediaMetadata::decode(&output.stdout)?;
        if metadata.is_live {
            return Err(MediaForgeError::LiveStream.into());
        }
        if metadata.is_entirely_drm() {
            return Err(MediaForgeError::DrmProtected.into());
        }
        Ok(metadata)
    }

    /// Downloads into `directory`, streaming yt-dlp's output to `on_event`.
    #[allow(clippy::too_many_arguments)]
    pub async fn download(
        &self,
        url: &ValidatedUrl,
        output: &OutputSpec,
        limits: DownloadLimits,
        embed_metadata: bool,
        video_compatibility: VideoCompatibility,
        directory: &Path,
        cancel: &CancellationToken,
        on_event: impl FnMut(ProcessEvent),
    ) -> Result<ProcessExit, Failure> {
        let arguments = download_arguments(url, output, limits, embed_metadata, video_compatibility, self.ffmpeg_directory.as_deref());
        let mut request = ProcessRequest::new(&self.executable, arguments);
        request.env = self.environment();
        request.working_directory = Some(directory.to_path_buf());
        request.timeout = Some(Duration::from_secs(6 * 60 * 60));
        // yt-dlp reports progress several times a second; ten silent minutes means it has hung.
        request.idle_timeout = Some(Duration::from_secs(600));
        request.max_output_bytes = 256 * 1024 * 1024;
        process::run_streaming(&request, cancel, on_event).await.map_err(translate)
    }
}

pub fn translate(error: ProcessError) -> Failure {
    match error {
        ProcessError::ExecutableNotFound(_) => MediaForgeError::ToolMissing(Tool::YtDlp).into(),
        ProcessError::LaunchFailed(detail) => MediaForgeError::DownloadFailed(format!("yt-dlp couldn't be started. {detail}")).into(),
        ProcessError::TimedOut(_) | ProcessError::Stalled(_) => MediaForgeError::TimedOut.into(),
        ProcessError::OutputLimitExceeded(_) => MediaForgeError::InvalidMetadata.into(),
        ProcessError::Cancelled => Failure::Cancelled,
    }
}

/// The media file a finished download left behind, ignoring sidecars and partial files.
pub fn locate_download(directory: &Path) -> Option<PathBuf> {
    const IGNORED: &[&str] = &["json", "part", "ytdl", "temp", "tmp"];
    let prefix = format!("{OUTPUT_STEM}.");
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ext = Path::new(&name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            name.starts_with(&prefix) && !IGNORED.contains(&ext.as_str()) && entry.file_type().is_ok_and(|t| t.is_file())
        })
        .max_by_key(|entry| entry.metadata().map(|m| m.len()).unwrap_or(0))
        .map(|entry| entry.path())
}

// MARK: - Output parsing

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStatus {
    Downloading,
    Finished,
    Error,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YtDlpProgress {
    pub status: ProgressStatus,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub bytes_per_second: Option<f64>,
    pub seconds_remaining: Option<f64>,
}

impl YtDlpProgress {
    pub fn new(status: ProgressStatus) -> Self {
        Self { status, downloaded_bytes: None, total_bytes: None, bytes_per_second: None, seconds_remaining: None }
    }

    pub fn fraction(&self) -> Option<f64> {
        if self.status == ProgressStatus::Finished {
            return Some(1.0);
        }
        let (done, total) = (self.downloaded_bytes?, self.total_bytes?);
        if total == 0 {
            return None;
        }
        Some((done as f64 / total as f64).clamp(0.0, 1.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum YtDlpEvent {
    Progress(YtDlpProgress),
    /// A new file started downloading. Video and audio usually arrive as separate parts.
    Destination(String),
    /// The format IDs yt-dlp settled on, one per part.
    SelectedFormats(Vec<String>),
    InfoJsonWritten(String),
    PostProcessing(&'static str),
    AlreadyDownloaded,
    SizeLimitExceeded,
    /// The item is a live broadcast and was skipped by the `!is_live` filter.
    SkippedLive,
}

pub fn parse_line(line: &str) -> Option<YtDlpEvent> {
    if let Some(payload) = line.strip_prefix(PROGRESS_PREFIX) {
        return parse_progress(payload);
    }
    if let Some(rest) = line.strip_prefix("[download] Destination: ") {
        return Some(YtDlpEvent::Destination(rest.to_string()));
    }
    if let Some(rest) = line.strip_prefix("[info] Writing video metadata as JSON to: ") {
        return Some(YtDlpEvent::InfoJsonWritten(rest.to_string()));
    }
    if line.starts_with("[info] ")
        && let Some(index) = line.find(" format(s): ")
    {
        let ids: Vec<String> =
            line[index + " format(s): ".len()..].split(['+', ',']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        return (!ids.is_empty()).then_some(YtDlpEvent::SelectedFormats(ids));
    }
    if line.starts_with("[download] ") {
        if line.contains("larger than max-filesize") {
            return Some(YtDlpEvent::SizeLimitExceeded);
        }
        if line.contains("does not pass filter") {
            return Some(YtDlpEvent::SkippedLive);
        }
        if line.ends_with("has already been downloaded") {
            return Some(YtDlpEvent::AlreadyDownloaded);
        }
    }
    let stage = if line.starts_with("[Merger]") {
        "Merging video and audio"
    } else if line.starts_with("[VideoRemuxer]") {
        "Repackaging as MP4"
    } else if line.starts_with("[VideoConvertor]") {
        "Converting video"
    } else if line.starts_with("[Metadata]") {
        "Writing metadata"
    } else if line.starts_with("[Fixup") {
        "Finishing up"
    } else {
        return None;
    };
    Some(YtDlpEvent::PostProcessing(stage))
}

fn parse_progress(payload: &str) -> Option<YtDlpEvent> {
    let fields: Vec<&str> = payload.split('|').collect();
    if fields.len() < 6 {
        return None;
    }
    let number = |index: usize| fields[index].trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
    let bytes = |index: usize| number(index).map(|v| v.min(9.0e15) as u64);
    let status = match fields[0].trim() {
        "downloading" => ProgressStatus::Downloading,
        "finished" => ProgressStatus::Finished,
        "error" => ProgressStatus::Error,
        _ => ProgressStatus::Other,
    };
    Some(YtDlpEvent::Progress(YtDlpProgress {
        status,
        downloaded_bytes: bytes(1),
        total_bytes: bytes(2).or_else(|| bytes(3)),
        bytes_per_second: number(4),
        seconds_remaining: number(5),
    }))
}

/// Combines per-file progress into one figure when a download arrives as several parts.
#[derive(Debug, Clone)]
pub struct ProgressAggregator {
    weights: Vec<f64>,
    part_index: isize,
    high_water_mark: f64,
}

impl Default for ProgressAggregator {
    fn default() -> Self {
        Self { weights: vec![1.0], part_index: -1, high_water_mark: 0.0 }
    }
}

impl ProgressAggregator {
    /// Call when yt-dlp announces its format selection. `sizes` are byte counts where known.
    pub fn set_parts(&mut self, sizes: &[Option<u64>]) {
        if sizes.is_empty() {
            return;
        }
        let known: Vec<u64> = sizes.iter().flatten().copied().filter(|s| *s > 0).collect();
        self.weights = if known.len() == sizes.len() {
            let total: u64 = known.iter().sum();
            sizes.iter().map(|s| s.unwrap_or(0) as f64 / total as f64).collect()
        } else if sizes.len() == 2 {
            // Unknown sizes: the first part is the video stream, which dwarfs the audio that follows.
            vec![0.9, 0.1]
        } else {
            vec![1.0 / sizes.len() as f64; sizes.len()]
        };
    }

    pub fn begin_part(&mut self) {
        self.part_index += 1;
    }

    /// Overall completion in 0...1, or `None` while the current part's size is unknown. Never moves backwards.
    pub fn overall_fraction(&mut self, progress: &YtDlpProgress) -> Option<f64> {
        let part = progress.fraction()?;
        let index = self.part_index.max(0) as usize;
        // More parts than announced: hold position rather than guess.
        if index >= self.weights.len() {
            return Some(self.high_water_mark);
        }
        let completed: f64 = self.weights[..index].iter().sum();
        self.high_water_mark = self.high_water_mark.max((completed + self.weights[index] * part).min(1.0));
        Some(self.high_water_mark)
    }
}

// MARK: - Errors

/// Maps yt-dlp's stderr onto errors a person can act on.
pub fn classify(lines: &[String]) -> MediaForgeError {
    let raw = lines.iter().rev().find(|l| l.starts_with("ERROR:")).or_else(|| lines.iter().rev().find(|l| !l.trim().is_empty()));
    let Some(raw) = raw else { return MediaForgeError::DownloadFailed(String::new()) };
    let text = raw.to_lowercase();
    let mentions = |needles: &[&str]| needles.iter().any(|n| text.contains(n));

    if mentions(&["ffmpeg not found", "ffprobe not found", "ffmpeg is not installed"]) {
        return MediaForgeError::ToolMissing(Tool::Ffmpeg);
    }
    if mentions(&["drm"]) {
        return MediaForgeError::DrmProtected;
    }
    if mentions(&["unsupported url", "is not a valid url", "no suitable extractor"]) {
        return MediaForgeError::UnsupportedSource;
    }
    if mentions(&[
        "sign in",
        "log in",
        "login required",
        "logged in",
        "members-only",
        "members only",
        "private video",
        "this video is private",
        "requires authentication",
        "authentication required",
        "registered users",
        "use --cookies",
        "--cookies-from-browser",
        "account credentials",
        "premium",
        "subscriber",
        "purchase",
        "rental",
        "age-restricted",
        "confirm your age",
        "http error 401",
        "password",
    ]) {
        return MediaForgeError::AuthenticationRequired;
    }
    if mentions(&[
        "not available in your country",
        "geo-restrict",
        "geo restrict",
        "georestrict",
        "not available from your location",
        "not available in your region",
        "blocked it in your country",
    ]) {
        return MediaForgeError::GeoRestricted;
    }
    if mentions(&["http error 429", "too many requests", "rate-limit", "rate limit"]) {
        return MediaForgeError::RateLimited;
    }
    if mentions(&["is live", "live event", "premieres in", "live stream", "will begin in"]) {
        return MediaForgeError::LiveStream;
    }
    if mentions(&["larger than max-filesize"]) {
        return MediaForgeError::ExceedsSizeLimit;
    }
    if mentions(&["requested format is not available", "no video formats found", "requested format not available"]) {
        return MediaForgeError::NoMatchingFormat;
    }
    if mentions(&[
        "http error 404",
        "http error 410",
        "video unavailable",
        "has been removed",
        "does not exist",
        "no longer available",
        "not found",
        "been deleted",
        "account has been terminated",
    ]) {
        return MediaForgeError::Unavailable;
    }
    if mentions(&["http error 403", "forbidden", "access denied"]) {
        return MediaForgeError::AccessDenied;
    }
    if mentions(&[
        "unable to download webpage",
        "name or service not known",
        "nodename nor servname",
        "temporary failure in name resolution",
        "timed out",
        "connection refused",
        "connection reset",
        "network is unreachable",
        "failed to resolve",
        "getaddrinfo",
        "transporterror",
        "unable to connect",
        "ssl:",
        "certificate verify failed",
        "remote end closed",
        "no connection could be made",
        "an existing connection was forcibly closed",
        "getaddrinfo failed",
    ]) {
        return MediaForgeError::Network;
    }
    if mentions(&["no space left", "not enough space on the disk"]) {
        return MediaForgeError::InsufficientDiskSpace;
    }
    MediaForgeError::DownloadFailed(cleaned(raw))
}

/// Strips yt-dlp's prefixes and boilerplate so the remaining sentence can be shown as-is.
pub fn cleaned(line: &str) -> String {
    let mut text = line.strip_prefix("ERROR: ").unwrap_or(line).to_string();
    // "[extractor] id: message" → "message"
    if text.starts_with('[')
        && let Some(close) = text.find(']')
    {
        let mut rest = text[close + 1..].trim_start().to_string();
        if let Some(colon) = rest.find(':')
            && !rest[..colon].contains(' ')
        {
            rest = rest[colon + 1..].trim_start().to_string();
        }
        text = rest;
    }
    for marker in [
        "; please report this issue",
        ". Please report this issue",
        " Confirm you are on the latest version",
        "; see  https://",
        "; see https://",
    ] {
        if let Some(index) = text.find(marker) {
            text.truncate(index);
        }
    }
    let mut text = text.trim().to_string();
    if text.chars().count() > 300 {
        text = text.chars().take(300).collect::<String>() + "…";
    }
    if let Some(last) = text.chars().last()
        && !".!?…".contains(last)
    {
        text.push('.');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AudioOptions, VideoOptions, VideoQuality};
    use crate::support::url_validator::validate;

    fn url() -> ValidatedUrl {
        validate("https://example.com/watch?v=abc").unwrap()
    }

    fn text(arguments: &[OsString]) -> Vec<String> {
        arguments.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn value_after(arguments: &[String], flag: &str) -> Option<String> {
        arguments.iter().position(|a| a == flag).and_then(|i| arguments.get(i + 1)).cloned()
    }

    fn download(output: OutputSpec, limits: DownloadLimits, ffmpeg: Option<&Path>) -> Vec<String> {
        text(&download_arguments(&url(), &output, limits, true, VideoCompatibility::Original, ffmpeg))
    }

    fn audio() -> OutputSpec {
        OutputSpec::Audio(AudioOptions::default())
    }

    fn video(quality: VideoQuality) -> OutputSpec {
        OutputSpec::Video(VideoOptions { quality })
    }

    #[test]
    fn the_link_is_always_the_final_positional_argument() {
        for output in [audio(), video(VideoQuality::P1080)] {
            let arguments = download(output, DownloadLimits::default(), None);
            assert_eq!(&arguments[arguments.len() - 2..], ["--", url().as_str()]);
            assert!(!arguments[..arguments.len() - 2].contains(&url().as_str().to_string()));
        }
        let metadata = text(&metadata_arguments(&url()));
        assert_eq!(&metadata[metadata.len() - 2..], ["--", url().as_str()]);
    }

    #[test]
    fn every_invocation_ignores_user_config_plugins_cookies_and_geo_bypass() {
        for arguments in [
            text(&metadata_arguments(&url())),
            download(audio(), DownloadLimits::default(), None),
            download(video(VideoQuality::Best), DownloadLimits::default(), None),
        ] {
            for flag in ["--ignore-config", "--no-plugin-dirs", "--no-cookies", "--no-cookies-from-browser", "--no-playlist"] {
                assert!(arguments.contains(&flag.to_string()), "{flag} missing");
            }
            assert_eq!(value_after(&arguments, "--xff").as_deref(), Some("never"));
        }
    }

    #[test]
    fn never_passes_options_that_weaken_security_or_bypass_restrictions() {
        let forbidden = [
            "--no-check-certificates",
            "--no-check-certificate",
            "--cookies",
            "--cookies-from-browser",
            "--username",
            "--password",
            "--netrc",
            "--netrc-cmd",
            "--exec",
            "--exec-before-download",
            "--allow-unplayable-formats",
            "--geo-bypass",
            "--geo-bypass-country",
            "--geo-verification-proxy",
            "--enable-file-urls",
            "--legacy-server-connect",
            "--prefer-insecure",
            "--video-password",
            "--ap-username",
            "--ap-password",
            "--plugin-dirs",
            "--config-locations",
            "--batch-file",
        ];
        let limits = DownloadLimits { rate_limit: Some(2_000_000), max_file_size: Some(1_000_000_000) };
        for output in [audio(), video(VideoQuality::Best)] {
            let arguments = download(output, limits, Some(Path::new("/opt/homebrew/bin")));
            assert!(forbidden.iter().all(|f| !arguments.contains(&f.to_string())));
        }
        let metadata = text(&metadata_arguments(&url()));
        assert!(forbidden.iter().all(|f| !metadata.contains(&f.to_string())));
    }

    #[test]
    fn audio_jobs_fetch_the_best_audio_and_video_jobs_produce_mp4() {
        let audio = download(audio(), DownloadLimits::default(), None);
        assert_eq!(value_after(&audio, "--format").as_deref(), Some("bestaudio/best"));
        assert!(!audio.contains(&"--merge-output-format".to_string()));

        let video_720 = download(video(VideoQuality::P720), DownloadLimits::default(), None);
        assert_eq!(value_after(&video_720, "--format").as_deref(), Some("bv*+ba/b[vcodec!=?none]"));
        assert_eq!(value_after(&video_720, "--format-sort").as_deref(), Some("res:720,fps,vcodec:h264,acodec:aac"));
        assert_eq!(value_after(&video_720, "--merge-output-format").as_deref(), Some("mp4"));
        assert_eq!(value_after(&video_720, "--remux-video").as_deref(), Some("mp4"));
        let best = download(video(VideoQuality::Best), DownloadLimits::default(), None);
        assert_eq!(value_after(&best, "--format-sort").as_deref(), Some("res,fps,vcodec:h264,acodec:aac"));

        // When video will be converted, AV1 is passed over while anything else is on offer.
        let converting =
            text(&download_arguments(&url(), &video(VideoQuality::P2160), DownloadLimits::default(), true, VideoCompatibility::H264, None));
        assert_eq!(value_after(&converting, "--format").as_deref(), Some("bv*[vcodec!^=?av01]+ba/bv*+ba/b[vcodec!=?none]"));
    }

    #[test]
    fn limits_timeouts_and_the_live_filter_are_applied() {
        let limits = DownloadLimits { rate_limit: Some(5_000_000), max_file_size: Some(2_000_000_000) };
        let arguments = download(audio(), limits, Some(Path::new("/opt/homebrew/bin")));
        assert_eq!(value_after(&arguments, "--limit-rate").as_deref(), Some("5000000"));
        assert_eq!(value_after(&arguments, "--max-filesize").as_deref(), Some("2000000000"));
        assert_eq!(value_after(&arguments, "--socket-timeout").as_deref(), Some("30"));
        assert_eq!(value_after(&arguments, "--ffmpeg-location").as_deref(), Some("/opt/homebrew/bin"));
        assert_eq!(value_after(&arguments, "--match-filters").as_deref(), Some("!is_live"));
        assert_eq!(value_after(&arguments, "--playlist-items").as_deref(), Some("1"));
        let unlimited = download(audio(), DownloadLimits::default(), None);
        assert!(!unlimited.contains(&"--limit-rate".to_string()));
        assert!(!unlimited.contains(&"--max-filesize".to_string()));
    }

    #[test]
    fn the_output_template_is_fixed() {
        let arguments = download(audio(), DownloadLimits::default(), None);
        assert_eq!(value_after(&arguments, "--output").as_deref(), Some("source.%(ext)s"));
        assert!(!arguments.contains(&"--paths".to_string()));
    }

    fn progress(status: ProgressStatus, done: Option<u64>, total: Option<u64>) -> YtDlpProgress {
        YtDlpProgress { status, downloaded_bytes: done, total_bytes: total, bytes_per_second: None, seconds_remaining: None }
    }

    #[test]
    fn parses_progress_lines_tolerating_na_and_float_byte_counts() {
        assert_eq!(
            parse_line("MFP|downloading|1048576|4194304|NA|524288.5|6"),
            Some(YtDlpEvent::Progress(YtDlpProgress {
                status: ProgressStatus::Downloading,
                downloaded_bytes: Some(1_048_576),
                total_bytes: Some(4_194_304),
                bytes_per_second: Some(524_288.5),
                seconds_remaining: Some(6.0),
            }))
        );
        assert_eq!(
            parse_line("MFP|downloading|500.0|NA|2000.0|NA|NA"),
            Some(YtDlpEvent::Progress(progress(ProgressStatus::Downloading, Some(500), Some(2000))))
        );
        assert_eq!(parse_line("MFP|garbage"), None);
        assert_eq!(
            parse_line("MFP|downloading|-5|inf|nan|NA|NA"),
            Some(YtDlpEvent::Progress(progress(ProgressStatus::Downloading, None, None)))
        );
    }

    #[test]
    fn computes_fractions_safely() {
        assert_eq!(progress(ProgressStatus::Downloading, Some(50), Some(200)).fraction(), Some(0.25));
        assert_eq!(progress(ProgressStatus::Downloading, Some(500), Some(200)).fraction(), Some(1.0));
        assert_eq!(progress(ProgressStatus::Downloading, Some(50), None).fraction(), None);
        assert_eq!(progress(ProgressStatus::Downloading, Some(50), Some(0)).fraction(), None);
        assert_eq!(progress(ProgressStatus::Finished, None, None).fraction(), Some(1.0));
    }

    #[test]
    fn recognises_the_informational_lines_the_pipeline_depends_on() {
        assert_eq!(parse_line("[download] Destination: source.f137.mp4"), Some(YtDlpEvent::Destination("source.f137.mp4".into())));
        assert_eq!(
            parse_line("[info] abc123: Downloading 1 format(s): 137+140"),
            Some(YtDlpEvent::SelectedFormats(vec!["137".into(), "140".into()]))
        );
        assert_eq!(parse_line("[info] clip: Downloading 1 format(s): mp4"), Some(YtDlpEvent::SelectedFormats(vec!["mp4".into()])));
        assert_eq!(
            parse_line("[info] Writing video metadata as JSON to: source.info.json"),
            Some(YtDlpEvent::InfoJsonWritten("source.info.json".into()))
        );
        assert_eq!(parse_line("[Merger] Merging formats into \"source.mp4\""), Some(YtDlpEvent::PostProcessing("Merging video and audio")));
        assert_eq!(parse_line("[VideoRemuxer] Remuxing video from webm to mp4"), Some(YtDlpEvent::PostProcessing("Repackaging as MP4")));
        assert_eq!(
            parse_line("[download] File is larger than max-filesize (900 bytes > 100 bytes). Aborting."),
            Some(YtDlpEvent::SizeLimitExceeded)
        );
        assert_eq!(parse_line("[download] Some Stream does not pass filter (!is_live), skipping .."), Some(YtDlpEvent::SkippedLive));
        assert_eq!(parse_line("[download] source.m4a has already been downloaded"), Some(YtDlpEvent::AlreadyDownloaded));
        assert_eq!(parse_line("[youtube] abc123: Downloading webpage"), None);
    }

    #[test]
    fn weights_multi_part_downloads_and_never_reports_backwards_progress() {
        let mut aggregator = ProgressAggregator::default();
        aggregator.set_parts(&[Some(900), Some(100)]);
        aggregator.begin_part();
        assert_eq!(aggregator.overall_fraction(&progress(ProgressStatus::Downloading, Some(450), Some(900))), Some(0.45));
        assert_eq!(aggregator.overall_fraction(&progress(ProgressStatus::Finished, None, None)), Some(0.9));
        aggregator.begin_part();
        assert_eq!(aggregator.overall_fraction(&progress(ProgressStatus::Downloading, Some(0), Some(100))), Some(0.9));
        let half = aggregator.overall_fraction(&progress(ProgressStatus::Downloading, Some(50), Some(100))).unwrap();
        assert!((half - 0.95).abs() < 0.0001);
        assert_eq!(aggregator.overall_fraction(&progress(ProgressStatus::Finished, None, None)), Some(1.0));
        aggregator.begin_part();
        assert_eq!(aggregator.overall_fraction(&progress(ProgressStatus::Downloading, Some(1), Some(100))), Some(1.0));
    }

    #[test]
    fn falls_back_to_sensible_weights_when_sizes_are_unknown() {
        let mut single = ProgressAggregator::default();
        single.begin_part();
        assert_eq!(single.overall_fraction(&progress(ProgressStatus::Downloading, Some(1), Some(4))), Some(0.25));
        assert_eq!(single.overall_fraction(&progress(ProgressStatus::Downloading, Some(1), None)), None);
        let mut pair = ProgressAggregator::default();
        pair.set_parts(&[None, None]);
        pair.begin_part();
        assert_eq!(pair.overall_fraction(&progress(ProgressStatus::Finished, None, None)), Some(0.9));
    }

    #[test]
    fn maps_failures_onto_actionable_errors() {
        let cases: &[(&str, MediaForgeError)] = &[
            ("ERROR: Unsupported URL: https://example.com/page", MediaForgeError::UnsupportedSource),
            ("ERROR: [DRM] The requested site is known to use DRM protection. It will NOT be supported.", MediaForgeError::DrmProtected),
            ("ERROR: [youtube] abc: This video is DRM protected", MediaForgeError::DrmProtected),
            (
                "ERROR: [youtube] abc: Sign in to confirm your age. This video may be inappropriate for some users.",
                MediaForgeError::AuthenticationRequired,
            ),
            (
                "ERROR: [youtube] abc: Private video. Sign in if you've been granted access to this video",
                MediaForgeError::AuthenticationRequired,
            ),
            (
                "ERROR: [vimeo] 123: This video is only available for registered users. Use --cookies-from-browser",
                MediaForgeError::AuthenticationRequired,
            ),
            (
                "ERROR: [youtube] abc: Join this channel to get access to members-only content like this video",
                MediaForgeError::AuthenticationRequired,
            ),
            ("ERROR: [site] abc: This video is not available in your country", MediaForgeError::GeoRestricted),
            (
                "ERROR: [youtube] abc: The uploader has not made this video available in your country\nERROR: geo-restricted",
                MediaForgeError::GeoRestricted,
            ),
            ("ERROR: unable to download video data: HTTP Error 429: Too Many Requests", MediaForgeError::RateLimited),
            ("ERROR: [youtube] abc: This live event will begin in 3 hours.", MediaForgeError::LiveStream),
            ("ERROR: [youtube] abc: Video unavailable. This video has been removed by the uploader", MediaForgeError::Unavailable),
            (
                "ERROR: [generic] Unable to download webpage: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)",
                MediaForgeError::Unavailable,
            ),
            ("ERROR: unable to download video data: HTTP Error 403: Forbidden", MediaForgeError::AccessDenied),
            (
                "ERROR: [generic] Unable to download webpage: <urlopen error [Errno 8] nodename nor servname provided, or not known>",
                MediaForgeError::Network,
            ),
            ("ERROR: [generic] Unable to download webpage: [Errno 61] Connection refused", MediaForgeError::Network),
            (
                "ERROR: [generic] Unable to download webpage: [WinError 10061] No connection could be made because the target machine actively refused it",
                MediaForgeError::Network,
            ),
            (
                "ERROR: [youtube] abc: Requested format is not available. Use --list-formats for a list of available formats",
                MediaForgeError::NoMatchingFormat,
            ),
            (
                "ERROR: You have requested merging of multiple formats but ffmpeg is not installed. Aborting",
                MediaForgeError::ToolMissing(Tool::Ffmpeg),
            ),
            ("ERROR: unable to write data: [Errno 28] No space left on device", MediaForgeError::InsufficientDiskSpace),
        ];
        for (stderr, expected) in cases {
            let mut lines = vec!["WARNING: something unrelated".to_string()];
            lines.extend(stderr.split('\n').map(str::to_string));
            assert_eq!(&classify(&lines), expected, "{stderr}");
        }
    }

    #[test]
    fn unknown_failures_keep_yt_dlps_own_explanation_cleaned_up() {
        let error = classify(&["ERROR: [somesite] xyz: Something odd happened; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template".into()]);
        assert_eq!(error, MediaForgeError::DownloadFailed("Something odd happened.".into()));
        assert_eq!(classify(&[]), MediaForgeError::DownloadFailed(String::new()));
    }

    #[test]
    fn locates_the_finished_file_ignoring_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source.info.json"), "{}").unwrap();
        std::fs::write(dir.path().join("source.webm.part"), "partial").unwrap();
        std::fs::write(dir.path().join("source.m4a"), "audio data").unwrap();
        std::fs::write(dir.path().join("other.mp3"), "not ours, and bigger than the rest").unwrap();
        assert_eq!(locate_download(dir.path()), Some(dir.path().join("source.m4a")));
    }
}
