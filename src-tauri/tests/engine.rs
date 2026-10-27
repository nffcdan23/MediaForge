//! End-to-end tests of the engine with the real tools: conversions, downloads through yt-dlp from a server on
//! the loopback interface, video conversion with the computer's encoder, failures, retries and cancellation.
//!
//! Tools are found in `MEDIAFORGE_TEST_TOOLS` (a folder holding ffmpeg, ffprobe and yt-dlp, as CI provides),
//! then in the standard install locations. Tests that need a missing tool say so and pass without running,
//! as the Swift suite's `.enabled(if:)` tests did.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mediaforge_lib::app::local_server::{LocalServer, tone_wav};
use mediaforge_lib::engine::job::{Job, JobSnapshot, JobSource, JobState};
use mediaforge_lib::engine::queue::{JobEnvironment, JobQueue, QueueObserver};
use mediaforge_lib::error::MediaForgeError;
use mediaforge_lib::media::ffmpeg::Ffmpeg;
use mediaforge_lib::media::ytdlp::YtDlp;
use mediaforge_lib::models::{
    AudioBitrate, AudioFormat, AudioOptions, BitDepth, DownloadLimits, ImageFormat, ImageOptions, ImageQuality, OutputSpec, SampleRate,
    VideoCompatibility, VideoOptions, VideoQuality,
};
use mediaforge_lib::process::{self, ProcessRequest};
use mediaforge_lib::support::url_validator;
use mediaforge_lib::tools::{self, BundledTools, Tool, ToolLocator, Toolchain};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// MARK: - Harness

fn toolchain() -> Toolchain {
    // A list of folders, separated as PATH is on this platform.
    let given: Vec<PathBuf> = std::env::var_os("MEDIAFORGE_TEST_TOOLS").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    let mut search = given.clone();
    search.extend(tools::default_search_directories());
    let locator = ToolLocator { search_directories: search, overrides: Default::default(), bundled: BundledTools::default() };
    Toolchain {
        ytdlp: locator.locate(Tool::YtDlp).ok(),
        ffmpeg: locator.locate(Tool::Ffmpeg).ok(),
        ffprobe: locator.locate(Tool::Ffprobe).ok(),
        support_directories: given,
    }
}

fn has_ffmpeg() -> bool {
    let t = toolchain();
    t.ffmpeg.is_some() && t.ffprobe.is_some()
}

fn has_download_stack() -> bool {
    has_ffmpeg() && toolchain().ytdlp.is_some()
}

macro_rules! require {
    ($condition:expr, $what:literal) => {
        if !$condition {
            eprintln!("skipped: {} not available", $what);
            return;
        }
    };
}

#[derive(Default)]
struct Recorder {
    finished: Mutex<Vec<Job>>,
    max_active: Mutex<usize>,
    active: Mutex<std::collections::HashSet<Uuid>>,
}

impl QueueObserver for Recorder {
    fn job_changed(&self, job: &JobSnapshot) {
        let json = serde_json::to_value(job).unwrap();
        let state = json["state"]["type"].as_str().unwrap_or("").to_string();
        let mut active = self.active.lock().unwrap();
        if ["preparing", "downloading", "processing", "converting", "finalizing"].contains(&state.as_str()) {
            active.insert(job.id);
        } else {
            active.remove(&job.id);
        }
        let mut max = self.max_active.lock().unwrap();
        *max = (*max).max(active.len());
    }
    fn jobs_removed(&self, _ids: &[Uuid]) {}
    fn job_finished(&self, job: &Job) {
        self.finished.lock().unwrap().push(job.clone());
    }
}

struct Harness {
    queue: JobQueue,
    recorder: Arc<Recorder>,
    destination: PathBuf,
    work_root: PathBuf,
    _dir: tempfile::TempDir,
}

fn harness_with(max: usize, video_compatibility: VideoCompatibility, limits: DownloadLimits, toolchain: Toolchain) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("Out");
    let work_root = dir.path().join("Work");
    let recorder = Arc::new(Recorder::default());
    let environment = JobEnvironment {
        toolchain,
        destination: destination.clone(),
        limits,
        embed_metadata: true,
        video_compatibility,
        image_options: ImageOptions::default(),
        work_root: work_root.clone(),
    };
    let queue = JobQueue::new(max, move || environment.clone(), recorder.clone());
    Harness { queue, recorder, destination, work_root, _dir: dir }
}

fn harness() -> Harness {
    harness_with(3, VideoCompatibility::H264, DownloadLimits::default(), toolchain())
}

impl Harness {
    async fn run(&self, jobs: Vec<Job>) -> Vec<Job> {
        let ids: Vec<Uuid> = jobs.iter().map(|j| j.id).collect();
        self.queue.enqueue(jobs);
        tokio::time::timeout(Duration::from_secs(240), self.queue.wait_until_idle()).await.expect("the queue never went idle");
        ids.iter().map(|id| self.queue.job(*id).expect("job vanished")).collect()
    }

    fn work_is_clean(&self) -> bool {
        std::fs::read_dir(&self.work_root).map(|d| d.count() == 0).unwrap_or(true)
    }
}

fn audio(format: AudioFormat) -> OutputSpec {
    OutputSpec::Audio(AudioOptions { format, ..AudioOptions::default() })
}

fn convert_job(file: &Path, output: OutputSpec) -> Job {
    let title = file.file_stem().unwrap().to_string_lossy().into_owned();
    Job::new(JobSource::File(file.to_path_buf()), output, title)
}

fn download_job(url: &str, output: OutputSpec) -> Job {
    let url = url_validator::validate(url).expect("valid test link");
    let title = url.display_name();
    Job::new(JobSource::Remote(url), output, title)
}

fn error_of(job: &Job) -> MediaForgeError {
    match &job.state {
        JobState::Failed(error) => error.clone(),
        other => panic!("expected a failure, got {other:?} (log: {:?})", job.log),
    }
}

fn assert_completed(job: &Job) -> PathBuf {
    match &job.state {
        JobState::Completed => job.output_path.clone().expect("a completed job has a file"),
        other => panic!("{} ended {other:?}; log: {:?}", job.title, job.log),
    }
}

async fn probe(path: &Path) -> mediaforge_lib::media::ffmpeg::MediaProbe {
    let t = toolchain();
    let service = Ffmpeg { ffmpeg: t.ffmpeg.unwrap(), ffprobe: t.ffprobe.unwrap() };
    service.probe(path, &CancellationToken::new()).await.expect("probe")
}

fn write_tone(dir: &Path, name: &str, seconds: f64) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, tone_wav(seconds, 44_100, 440.0)).unwrap();
    path
}

/// Runs FFmpeg directly to make test media. Returns false when this FFmpeg can't (for example no lavfi).
async fn ffmpeg_make(args: &[&str], output: &Path) -> bool {
    let Some(ffmpeg) = toolchain().ffmpeg else { return false };
    let mut arguments: Vec<std::ffi::OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-y"].iter().map(Into::into).collect();
    arguments.extend(args.iter().map(Into::into));
    arguments.push(output.as_os_str().to_owned());
    let mut request = ProcessRequest::new(ffmpeg, arguments);
    request.timeout = Some(Duration::from_secs(180));
    matches!(process::run_collect(&request, &CancellationToken::new()).await, Ok(out) if out.exit.success()) && output.is_file()
}

// MARK: - Conversions

#[tokio::test(flavor = "multi_thread")]
async fn converts_a_local_file_to_every_format() {
    require!(has_ffmpeg(), "FFmpeg");
    let h = harness();
    let media = tempfile::tempdir().unwrap();
    let tone = write_tone(media.path(), "Licensed Bed.wav", 3.0);
    let cases = [
        (AudioFormat::Mp3, BitDepth::Pcm16, "mp3"),
        (AudioFormat::Wav, BitDepth::Pcm24, "pcm_s24le"),
        (AudioFormat::Aac, BitDepth::Pcm16, "aac"),
        (AudioFormat::M4a, BitDepth::Pcm16, "aac"),
        (AudioFormat::Flac, BitDepth::Pcm24, "flac"),
        (AudioFormat::Aiff, BitDepth::Pcm16, "pcm_s16be"),
    ];
    let jobs: Vec<Job> = cases
        .iter()
        .map(|(format, depth, _)| {
            convert_job(
                &tone,
                OutputSpec::Audio(AudioOptions {
                    format: *format,
                    bitrate: AudioBitrate::Kbps192,
                    sample_rate: SampleRate::Hz48000,
                    bit_depth: *depth,
                }),
            )
        })
        .collect();
    let done = h.run(jobs).await;
    for (job, (format, _, codec)) in done.iter().zip(cases) {
        let path = assert_completed(job);
        assert_eq!(path.parent().unwrap(), h.destination);
        assert_eq!(path.extension().unwrap().to_string_lossy(), format.extension());
        let probe = probe(&path).await;
        let stream = probe.audio.expect("audio");
        assert_eq!(stream.codec, codec, "{format:?}");
        assert_eq!(stream.sample_rate, Some(48_000), "{format:?}");
        assert!(job.output_size.unwrap_or(0) > 0);
    }
    assert_eq!(h.recorder.finished.lock().unwrap().len(), cases.len());
    assert!(h.work_is_clean());
}

#[tokio::test(flavor = "multi_thread")]
async fn never_overwrites_repeated_conversions_get_numbered_names() {
    require!(has_ffmpeg(), "FFmpeg");
    let h = harness_with(1, VideoCompatibility::H264, DownloadLimits::default(), toolchain());
    let media = tempfile::tempdir().unwrap();
    let tone = write_tone(media.path(), "Theme.wav", 1.0);
    let done = h.run(vec![convert_job(&tone, audio(AudioFormat::Mp3)), convert_job(&tone, audio(AudioFormat::Mp3))]).await;
    let names: Vec<String> = done.iter().map(|j| assert_completed(j).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Theme.mp3", "Theme 2.mp3"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_conversions_respect_the_concurrency_limit() {
    require!(has_ffmpeg(), "FFmpeg");
    let h = harness_with(2, VideoCompatibility::H264, DownloadLimits::default(), toolchain());
    let media = tempfile::tempdir().unwrap();
    let jobs: Vec<Job> =
        (0..5).map(|i| convert_job(&write_tone(media.path(), &format!("Take {i}.wav"), 2.0), audio(AudioFormat::Flac))).collect();
    let done = h.run(jobs).await;
    assert!(done.iter().all(|j| j.state == JobState::Completed));
    let max = *h.recorder.max_active.lock().unwrap();
    assert!((1..=2).contains(&max), "{max} ran at once");
}

#[tokio::test(flavor = "multi_thread")]
async fn handles_awkward_file_names() {
    require!(has_ffmpeg(), "FFmpeg");
    let h = harness();
    let media = tempfile::tempdir().unwrap();
    let names = ["-rf --overwrite.wav", "pipe:1.wav", "Café – 東京 🎧.wav"];
    let jobs: Vec<Job> = names.iter().map(|n| convert_job(&write_tone(media.path(), n, 1.0), audio(AudioFormat::Mp3))).collect();
    for job in h.run(jobs).await {
        let path = assert_completed(&job);
        assert!(path.starts_with(&h.destination));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_missing_unreadable_and_silent_files_clearly() {
    require!(has_ffmpeg(), "FFmpeg");
    let h = harness();
    let media = tempfile::tempdir().unwrap();
    let missing = media.path().join("Gone.wav");
    let corrupt = media.path().join("Corrupt.mp3");
    std::fs::write(&corrupt, b"this is not audio at all").unwrap();
    let done = h.run(vec![convert_job(&missing, audio(AudioFormat::Mp3)), convert_job(&corrupt, audio(AudioFormat::Mp3))]).await;
    assert!(matches!(error_of(&done[0]), MediaForgeError::SourceFileMissing(_)));
    assert!(matches!(error_of(&done[1]), MediaForgeError::UnreadableFile(_)));

    let silent = media.path().join("Silent.mp4");
    if ffmpeg_make(&["-f", "lavfi", "-i", "color=c=black:s=320x240:d=1", "-c:v", "mpeg4"], &silent).await {
        let done = h.run(vec![convert_job(&silent, audio(AudioFormat::Mp3))]).await;
        assert_eq!(error_of(&done[0]), MediaForgeError::NoAudioStream);
    }
    assert!(h.work_is_clean());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_job_succeeds_on_retry_once_the_problem_is_fixed() {
    require!(has_ffmpeg(), "FFmpeg");
    let h = harness();
    let media = tempfile::tempdir().unwrap();
    let path = media.path().join("Late.wav");
    let job = convert_job(&path, audio(AudioFormat::Mp3));
    let id = job.id;
    let done = h.run(vec![job]).await;
    assert!(matches!(error_of(&done[0]), MediaForgeError::SourceFileMissing(_)));
    write_tone(media.path(), "Late.wav", 1.0);
    h.queue.retry(id);
    tokio::time::timeout(Duration::from_secs(60), h.queue.wait_until_idle()).await.unwrap();
    let job = h.queue.job(id).unwrap();
    assert_completed(&job);
    assert_eq!(job.attempt, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_a_missing_tool_and_an_unusable_destination() {
    let missing_tools = harness_with(1, VideoCompatibility::H264, DownloadLimits::default(), Toolchain::default());
    let media = tempfile::tempdir().unwrap();
    let tone = write_tone(media.path(), "Tone.wav", 0.5);
    let done = missing_tools.run(vec![convert_job(&tone, audio(AudioFormat::Mp3))]).await;
    assert_eq!(error_of(&done[0]), MediaForgeError::ToolMissing(Tool::Ffmpeg));

    let h = harness();
    std::fs::create_dir_all(h.destination.parent().unwrap()).unwrap();
    std::fs::write(&h.destination, "a file where the folder should be").unwrap();
    let done = h.run(vec![convert_job(&tone, audio(AudioFormat::Mp3))]).await;
    assert!(matches!(error_of(&done[0]), MediaForgeError::DestinationUnavailable(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_waiting_job_takes_it_out_of_line_and_retry_runs_it_again() {
    require!(has_ffmpeg(), "FFmpeg");
    let h = harness_with(1, VideoCompatibility::H264, DownloadLimits::default(), toolchain());
    let media = tempfile::tempdir().unwrap();
    let first = convert_job(&write_tone(media.path(), "First.wav", 4.0), audio(AudioFormat::Flac));
    let second = convert_job(&write_tone(media.path(), "Second.wav", 1.0), audio(AudioFormat::Mp3));
    let second_id = second.id;
    h.queue.enqueue(vec![first, second]);
    h.queue.cancel(second_id);
    assert_eq!(h.queue.job(second_id).unwrap().state, JobState::Cancelled);
    h.queue.wait_until_idle().await;
    h.queue.retry(second_id);
    h.queue.wait_until_idle().await;
    assert_completed(&h.queue.job(second_id).unwrap());
}

// MARK: - Downloads

#[tokio::test(flavor = "multi_thread")]
async fn looks_up_a_direct_media_link() {
    require!(has_download_stack(), "yt-dlp");
    let served = tempfile::tempdir().unwrap();
    write_tone(served.path(), "Spring Campaign.wav", 3.0);
    let server = LocalServer::start(served.path(), None).unwrap();
    let t = toolchain();
    let service = YtDlp {
        executable: t.ytdlp.unwrap(),
        ffmpeg_directory: t.ffmpeg.and_then(|p| p.parent().map(Path::to_path_buf)),
        support_directories: t.support_directories,
    };
    let url = url_validator::validate(&server.url("Spring%20Campaign.wav")).unwrap();
    let metadata = service.fetch_metadata(&url, &CancellationToken::new()).await.expect("lookup");
    assert_eq!(metadata.title, "Spring Campaign");
    assert!(!metadata.has_video());
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_audio_into_lossy_and_lossless_formats_with_tags() {
    require!(has_download_stack(), "yt-dlp");
    let h = harness();
    let served = tempfile::tempdir().unwrap();
    write_tone(served.path(), "Launch Film.wav", 3.0);
    let server = LocalServer::start(served.path(), None).unwrap();
    let url = server.url("Launch%20Film.wav");
    let done = h.run(vec![download_job(&url, audio(AudioFormat::Mp3)), download_job(&url, audio(AudioFormat::Flac))]).await;
    let mp3 = assert_completed(&done[0]);
    let flac = assert_completed(&done[1]);
    assert_eq!(probe(&mp3).await.audio.unwrap().codec, "mp3");
    assert_eq!(probe(&flac).await.audio.unwrap().codec, "flac");
    let tags = probe(&mp3).await;
    assert_eq!(tags.title.as_deref(), Some("Launch Film"));
    assert!(h.work_is_clean());
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_video_as_mp4_and_converts_formats_players_cant_open() {
    require!(has_download_stack(), "yt-dlp");
    let served = tempfile::tempdir().unwrap();
    let clip = served.path().join("Autumn Campaign.webm");
    let made = ffmpeg_make(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "1M",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-c:a",
            "libopus",
            "-shortest",
        ],
        &clip,
    )
    .await;
    if !made {
        eprintln!("skipped: this FFmpeg can't make a VP9 test clip");
        return;
    }
    let server = LocalServer::start(served.path(), None).unwrap();
    let url = server.url("Autumn%20Campaign.webm");

    let converting = harness();
    let done = converting.run(vec![download_job(&url, OutputSpec::Video(VideoOptions { quality: VideoQuality::Best }))]).await;
    let mp4 = assert_completed(&done[0]);
    let probe_converted = probe(&mp4).await;
    assert_eq!(probe_converted.video.as_ref().unwrap().codec, "h264", "log: {:?}", done[0].log);
    assert_eq!(probe_converted.video.unwrap().height, Some(720));
    assert!(converting.work_is_clean());

    let original = harness_with(1, VideoCompatibility::Original, DownloadLimits::default(), toolchain());
    let done = original.run(vec![download_job(&url, OutputSpec::Video(VideoOptions { quality: VideoQuality::Best }))]).await;
    let kept = assert_completed(&done[0]);
    assert_eq!(probe(&kept).await.video.unwrap().codec, "vp9");
}

#[tokio::test(flavor = "multi_thread")]
async fn asking_for_video_from_an_audio_only_source_fails_clearly() {
    require!(has_download_stack(), "yt-dlp");
    let h = harness();
    let served = tempfile::tempdir().unwrap();
    write_tone(served.path(), "Voice.wav", 1.0);
    let server = LocalServer::start(served.path(), None).unwrap();
    let done = h.run(vec![download_job(&server.url("Voice.wav"), OutputSpec::Video(VideoOptions { quality: VideoQuality::P1080 }))]).await;
    assert_eq!(error_of(&done[0]), MediaForgeError::NoMatchingFormat);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_pages_and_unreachable_servers_get_their_own_errors() {
    require!(has_download_stack(), "yt-dlp");
    let h = harness();
    let served = tempfile::tempdir().unwrap();
    let server = LocalServer::start(served.path(), None).unwrap();
    // A port nothing listens on: bind, note it, release it.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let done = h
        .run(vec![
            download_job(&server.url("missing.mp3"), audio(AudioFormat::Mp3)),
            download_job(&format!("http://127.0.0.1:{closed}/clip.mp3"), audio(AudioFormat::Mp3)),
        ])
        .await;
    assert_eq!(error_of(&done[0]), MediaForgeError::Unavailable);
    assert_eq!(error_of(&done[1]), MediaForgeError::Network);
    assert!(error_of(&done[1]).is_retryable());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_size_limit_stops_an_oversized_download() {
    require!(has_download_stack(), "yt-dlp");
    let h = harness_with(1, VideoCompatibility::H264, DownloadLimits { rate_limit: None, max_file_size: Some(10_000) }, toolchain());
    let served = tempfile::tempdir().unwrap();
    write_tone(served.path(), "Long.wav", 5.0);
    let server = LocalServer::start(served.path(), None).unwrap();
    let done = h.run(vec![download_job(&server.url("Long.wav"), audio(AudioFormat::Mp3))]).await;
    assert_eq!(error_of(&done[0]), MediaForgeError::ExceedsSizeLimit);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_can_be_cancelled_mid_transfer_leaving_nothing_behind() {
    require!(has_download_stack(), "yt-dlp");
    let h = harness();
    let served = tempfile::tempdir().unwrap();
    write_tone(served.path(), "Slow.wav", 30.0);
    let server = LocalServer::start(served.path(), Some(200_000)).unwrap();
    let job = download_job(&server.url("Slow.wav"), audio(AudioFormat::Mp3));
    let id = job.id;
    h.queue.enqueue(vec![job]);
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && h.queue.job(id).is_some_and(|j| j.state != JobState::Downloading) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    h.queue.cancel(id);
    tokio::time::timeout(Duration::from_secs(30), h.queue.wait_until_idle()).await.expect("cancellation took too long");
    assert_eq!(h.queue.job(id).unwrap().state, JobState::Cancelled);
    assert!(h.work_is_clean(), "working files were left behind");
    assert!(!h.destination.join("Slow.mp3").exists());
}

// MARK: - Images (no helper tools needed)

fn image_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/images").join(name)
}

fn image(format: ImageFormat) -> OutputSpec {
    OutputSpec::Image(ImageOptions { format, quality: ImageQuality::High })
}

fn image_kind(path: &Path) -> mediaforge_lib::media::image::SourceKind {
    mediaforge_lib::media::image::identify(path).expect("an image")
}

#[tokio::test(flavor = "multi_thread")]
async fn converts_webp_images_to_jpg_and_png() {
    use mediaforge_lib::media::image::SourceKind;
    let h = harness_with(2, VideoCompatibility::H264, DownloadLimits::default(), Toolchain::default());
    let pictures = tempfile::tempdir().unwrap();
    let photo = pictures.path().join("Harbour at Dusk.webp");
    std::fs::copy(image_fixture("photo-lossy.webp"), &photo).unwrap();
    let logo = pictures.path().join("logo.webp");
    std::fs::copy(image_fixture("logo-alpha.webp"), &logo).unwrap();
    let done = h
        .run(vec![
            convert_job(&photo, image(ImageFormat::Jpg)),
            convert_job(&photo, image(ImageFormat::Png)),
            convert_job(&logo, image(ImageFormat::Jpg)),
        ])
        .await;
    let paths: Vec<PathBuf> = done.iter().map(assert_completed).collect();
    let names: Vec<String> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Harbour at Dusk.jpg", "Harbour at Dusk.png", "logo.jpg"]);
    assert_eq!(paths.iter().map(|p| image_kind(p)).collect::<Vec<_>>(), [SourceKind::Jpeg, SourceKind::Png, SourceKind::Jpeg]);
    assert!(done.iter().all(|j| j.output_size.unwrap_or(0) > 0));
    // The originals are untouched.
    assert_eq!(std::fs::read(&photo).unwrap(), std::fs::read(image_fixture("photo-lossy.webp")).unwrap());
    assert!(h.work_is_clean());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_image_fails_clearly() {
    let h = harness_with(1, VideoCompatibility::H264, DownloadLimits::default(), Toolchain::default());
    let pictures = tempfile::tempdir().unwrap();
    let broken = pictures.path().join("broken.webp");
    let whole = std::fs::read(image_fixture("photo-lossy.webp")).unwrap();
    std::fs::write(&broken, &whole[..60]).unwrap();
    let done = h.run(vec![convert_job(&broken, image(ImageFormat::Jpg))]).await;
    assert!(matches!(error_of(&done[0]), MediaForgeError::UnreadableFile(_)));
    assert!(std::fs::read_dir(&h.destination).map(|d| d.count() == 0).unwrap_or(true));
    assert!(h.work_is_clean());
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_an_image_link_as_jpg_and_marks_it_as_downloaded() {
    let h = harness_with(2, VideoCompatibility::H264, DownloadLimits::default(), Toolchain::default());
    let served = tempfile::tempdir().unwrap();
    std::fs::copy(image_fixture("photo-lossy.webp"), served.path().join("Harbour.webp")).unwrap();
    std::fs::copy(image_fixture("logo-alpha.webp"), served.path().join("Logo.webp")).unwrap();
    let server = LocalServer::start(served.path(), None).unwrap();
    let done = h
        .run(vec![
            download_job(&server.url("Harbour.webp"), image(ImageFormat::Jpg)),
            // Queued as audio, as a batch of links is: an image link can only become an image.
            download_job(&server.url("Logo.webp"), audio(AudioFormat::Mp3)),
        ])
        .await;
    let photo = assert_completed(&done[0]);
    assert_eq!(photo.file_name().unwrap(), "Harbour.jpg");
    assert_eq!(image_kind(&photo), mediaforge_lib::media::image::SourceKind::Jpeg);
    let logo = assert_completed(&done[1]);
    assert_eq!(logo.file_name().unwrap(), "Logo.jpg");
    assert_eq!(done[1].output, OutputSpec::Image(ImageOptions::default()));
    assert!(h.work_is_clean());
    #[cfg(target_os = "macos")]
    {
        let marked = std::process::Command::new("/usr/bin/xattr").arg("-p").arg("com.apple.quarantine").arg(&photo).output().unwrap();
        assert!(marked.status.success(), "the download should carry the quarantine mark");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn image_links_respect_the_size_limit_and_missing_ones_say_so() {
    let limits = DownloadLimits { rate_limit: None, max_file_size: Some(100) };
    let h = harness_with(2, VideoCompatibility::H264, limits, Toolchain::default());
    let served = tempfile::tempdir().unwrap();
    std::fs::copy(image_fixture("logo-alpha.webp"), served.path().join("Logo.webp")).unwrap();
    let server = LocalServer::start(served.path(), None).unwrap();
    let done = h
        .run(vec![
            download_job(&server.url("Logo.webp"), image(ImageFormat::Png)),
            download_job(&server.url("gone.png"), image(ImageFormat::Png)),
        ])
        .await;
    assert_eq!(error_of(&done[0]), MediaForgeError::ExceedsSizeLimit);
    assert_eq!(error_of(&done[1]), MediaForgeError::Unavailable);
    assert!(h.work_is_clean());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_behind_a_link_without_a_file_name_still_becomes_an_image() {
    require!(has_download_stack(), "yt-dlp");
    let h = harness();
    let served = tempfile::tempdir().unwrap();
    std::fs::copy(image_fixture("photo-lossy.webp"), served.path().join("picture")).unwrap();
    let server = LocalServer::start(served.path(), None).unwrap();
    let done = h.run(vec![download_job(&server.url("picture"), audio(AudioFormat::Mp3))]).await;
    let path = assert_completed(&done[0]);
    assert_eq!(path.extension().unwrap(), "jpg");
    assert_eq!(image_kind(&path), mediaforge_lib::media::image::SourceKind::Jpeg);
    assert!(h.work_is_clean());
}
