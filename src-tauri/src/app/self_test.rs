//! `MediaForge --self-test <report.json>`: checks that an installed copy works on this computer, then quits.
//!
//! It drives the real engine through the same queue the UI uses: the bundled tools, conversions to every
//! format, image conversion and image links, downloads with yt-dlp, video conversion with this computer's
//! encoder, artwork, waveforms and cancellation. Media is generated on the spot and served from the loopback interface, so nothing outside
//! the computer is contacted. Settings, history and output go to a temporary folder that is removed
//! afterwards; the user's own are never touched.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::local_server::{self, LocalServer};
use super::state::AppState;
use crate::engine::job::{Job, JobSource, JobState};
use crate::media::ffmpeg::Ffmpeg;
use crate::media::image::{self as images, SourceKind};
use crate::media::image_link::{self, Probe};
use crate::media::ytdlp::YtDlp;
use crate::models::{
    AudioBitrate, AudioFormat, AudioOptions, BitDepth, ImageFormat, ImageOptions, ImageQuality, OutputSpec, SampleRate, VideoCompatibility,
    VideoOptions, VideoQuality,
};
use crate::process::{self, ProcessRequest};
use crate::support::url_validator;
use crate::tools::{Tool, ToolStatus};

pub fn requested() -> Option<PathBuf> {
    let arguments: Vec<String> = std::env::args().collect();
    let index = arguments.iter().position(|a| a == "--self-test")?;
    arguments.get(index + 1).map(PathBuf::from)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    name: String,
    ok: bool,
    /// False when the check couldn't run here (for example a format this build of FFmpeg can't generate).
    ran: bool,
    detail: String,
    seconds: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    ok: bool,
    platform: &'static str,
    version: String,
    build: String,
    executable: String,
    tools: Vec<ToolStatus>,
    checks: Vec<Check>,
}

struct Checks(Vec<Check>);

impl Checks {
    fn record(&mut self, name: &str, started: Instant, result: Result<String, String>) {
        let (ok, detail) = match result {
            Ok(detail) => (true, detail),
            Err(detail) => (false, detail),
        };
        self.0.push(Check { name: name.into(), ok, ran: true, detail, seconds: started.elapsed().as_secs_f64() });
    }

    fn skip(&mut self, name: &str, reason: &str) {
        self.0.push(Check { name: name.into(), ok: true, ran: false, detail: reason.into(), seconds: 0.0 });
    }
}

pub fn start(app: AppHandle, report_path: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        state.settings.lock().unwrap().modify(|s| {
            s.notifications_enabled = false;
            s.has_acknowledged_terms = true;
            s.video_compatibility = VideoCompatibility::H264;
        });
        let mut checks = Checks(Vec::new());
        run(&app, &state, &mut checks).await;
        let report = Report {
            ok: checks.0.iter().all(|c| c.ok),
            platform: super::commands::platform(),
            version: app.package_info().version.to_string(),
            build: super::commands::build_number(&app),
            executable: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
            tools: state.tools.statuses(),
            checks: checks.0,
        };
        let ok = report.ok;
        if let Ok(data) = serde_json::to_vec_pretty(&report) {
            let _ = std::fs::write(&report_path, data);
        }
        state.quitting.store(true, Ordering::SeqCst);
        state.clean_up();
        if let Some(scratch) = state.paths.data.parent()
            && scratch.file_name().is_some_and(|n| n.to_string_lossy().starts_with("MediaForge-SelfTest-"))
        {
            let _ = std::fs::remove_dir_all(scratch);
        }
        app.exit(if ok { 0 } else { 1 });
    });
}

async fn wait_for_job(state: &AppState, id: Uuid, limit: Duration) -> Option<Job> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(job) = state.queue.job(id)
            && job.state.is_finished()
        {
            return Some(job);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    state.queue.job(id)
}

fn describe(job: Option<Job>) -> Result<Job, String> {
    let job = job.ok_or("the job disappeared")?;
    match &job.state {
        JobState::Completed => Ok(job),
        JobState::Failed(error) => {
            Err(format!("{}: {} | log: {}", error.title(), error.message(), job.log.iter().cloned().collect::<Vec<_>>().join(" / ")))
        }
        other => Err(format!("ended in state {other:?}")),
    }
}

async fn probe_codecs(state: &AppState, path: &Path) -> Result<(Option<String>, Option<String>, Option<u32>), String> {
    let service = Ffmpeg {
        ffmpeg: state.tools.path(Tool::Ffmpeg).ok_or("FFmpeg missing")?,
        ffprobe: state.tools.path(Tool::Ffprobe).ok_or("FFprobe missing")?,
    };
    let probe = service.probe(path, &CancellationToken::new()).await.map_err(|e| format!("{e:?}"))?;
    Ok((probe.audio.map(|a| a.codec), probe.video.as_ref().map(|v| v.codec.clone()), probe.video.and_then(|v| v.height)))
}

async fn run(app: &AppHandle, state: &Arc<AppState>, checks: &mut Checks) {
    // The window and its script loaded: proves the platform's web view works.
    let started = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !state.frontend_ready.load(Ordering::SeqCst) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let ready = state.frontend_ready.load(Ordering::SeqCst);
    checks.record(
        "window",
        started,
        if ready { Ok("the interface loaded".into()) } else { Err("the interface did not load within 60 s".into()) },
    );

    // Tools, and their versions (which proves each one starts).
    let started = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let statuses = state.tools.statuses();
        if statuses.iter().all(|s| s.version.is_some()) || Instant::now() > deadline {
            let missing: Vec<String> = statuses
                .iter()
                .filter(|s| s.version.is_none())
                .map(|s| format!("{} ({})", s.name, s.error.clone().unwrap_or_else(|| "didn't start".into())))
                .collect();
            let summary: Vec<String> = statuses
                .iter()
                .map(|s| {
                    format!("{} {} {}", s.name, s.version.clone().unwrap_or_default(), if s.bundled { "(bundled)" } else { "(installed)" })
                })
                .collect();
            checks.record(
                "tools",
                started,
                if missing.is_empty() { Ok(summary.join(", ")) } else { Err(format!("not working: {}", missing.join(", "))) },
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let deno =
        state.tools.toolchain().support_directories.iter().any(|d| d.join(format!("deno{}", std::env::consts::EXE_SUFFIX)).is_file());
    checks.record(
        "deno",
        Instant::now(),
        if deno { Ok("JavaScript runtime for yt-dlp is bundled".into()) } else { Err("deno was not found beside the app".into()) },
    );

    let scratch = state.paths.data.parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir).join("media");
    let _ = std::fs::create_dir_all(&scratch);
    let tone = scratch.join("Self Test Tone.wav");
    if std::fs::write(&tone, local_server::tone_wav(4.0, 44_100, 440.0)).is_err() {
        checks.record("media", Instant::now(), Err("couldn't write the test tone".into()));
        return;
    }

    // Every audio format, through the same queue the UI uses.
    let formats = [
        (AudioFormat::Mp3, BitDepth::Pcm16, "mp3"),
        (AudioFormat::Wav, BitDepth::Pcm24, "pcm_s24le"),
        (AudioFormat::Aac, BitDepth::Pcm16, "aac"),
        (AudioFormat::M4a, BitDepth::Pcm16, "aac"),
        (AudioFormat::Flac, BitDepth::Pcm16, "flac"),
        (AudioFormat::Aiff, BitDepth::Pcm16, "pcm_s16be"),
    ];
    let mut mp3_output: Option<PathBuf> = None;
    for (format, depth, codec) in formats {
        let started = Instant::now();
        let options = AudioOptions { format, bitrate: AudioBitrate::Kbps192, sample_rate: SampleRate::Hz48000, bit_depth: depth };
        let mut job = Job::new(JobSource::File(tone.clone()), OutputSpec::Audio(options), format!("Self Test {}", format.display_name()));
        job.detail = Some("WAV".into());
        let id = job.id;
        state.queue.enqueue(vec![job]);
        let result = match describe(wait_for_job(state, id, Duration::from_secs(120)).await) {
            Ok(job) => {
                let path = job.output_path.clone().unwrap_or_default();
                match probe_codecs(state, &path).await {
                    Ok((Some(found), _, _)) if found == codec => {
                        if format == AudioFormat::Mp3 {
                            mp3_output = Some(path.clone());
                        }
                        Ok(format!(
                            "{} ({} bytes, {found})",
                            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            job.output_size.unwrap_or(0)
                        ))
                    }
                    Ok((found, _, _)) => Err(format!("expected {codec}, found {found:?}")),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        checks.record(&format!("convert {}", format.display_name()), started, result);
    }

    // Waveform and artwork for the preview player.
    if let (Some(mp3), Some(ffmpeg)) = (&mp3_output, state.tools.path(Tool::Ffmpeg)) {
        let started = Instant::now();
        let peaks = crate::artwork::waveform(&ffmpeg, mp3, Some(4.0), 180).await;
        let loudest = peaks.iter().copied().fold(0.0f32, f32::max);
        checks.record(
            "waveform",
            started,
            if peaks.len() == 180 && (loudest - 1.0).abs() < 0.001 {
                Ok("180 peaks".into())
            } else {
                Err(format!("{} peaks, loudest {loudest}", peaks.len()))
            },
        );
    }

    // Images: a WebP with a transparent half, made here, into JPG (transparency on white) and PNG (kept).
    let picture = scratch.join("Self Test Picture.webp");
    let have_picture = write_test_webp(&picture);
    if !have_picture {
        checks.record("convert image", Instant::now(), Err("couldn't write the test picture".into()));
    }
    for format in [ImageFormat::Jpg, ImageFormat::Png] {
        if !have_picture {
            break;
        }
        let started = Instant::now();
        let options = ImageOptions { format, quality: ImageQuality::High };
        let job =
            Job::new(JobSource::File(picture.clone()), OutputSpec::Image(options), format!("Self Test Picture {}", format.display_name()));
        let id = job.id;
        state.queue.enqueue(vec![job]);
        let result = describe(wait_for_job(state, id, Duration::from_secs(60)).await).and_then(|job| check_picture(&job, format));
        checks.record(&format!("convert image {}", format.display_name()), started, result);
    }
    if have_picture {
        let started = Instant::now();
        let thumbnail = state.artwork.local(&picture, None).await;
        checks.record(
            "image thumbnail",
            started,
            match thumbnail {
                Some(url) if url.starts_with("data:image/png") => Ok(format!("{} bytes", url.len())),
                other => Err(format!("unexpected thumbnail: {:?}", other.map(|u| u.chars().take(30).collect::<String>()))),
            },
        );
    }

    // Downloads through yt-dlp, from a server on this computer.
    let Ok(server) = LocalServer::start(&scratch, None) else {
        checks.record("download", Instant::now(), Err("couldn't start the local test server".into()));
        return;
    };
    let tone_url = server.url("Self%20Test%20Tone.wav");
    let started = Instant::now();
    let toolchain = state.tools.toolchain();
    let lookup = match (&toolchain.ytdlp, url_validator::validate(&tone_url)) {
        (Some(ytdlp), Ok(url)) => {
            let service = YtDlp {
                executable: ytdlp.clone(),
                ffmpeg_directory: toolchain.ffmpeg.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)),
                support_directories: toolchain.support_directories.clone(),
            };
            match service.fetch_metadata(&url, &CancellationToken::new()).await {
                Ok(metadata) => Ok(format!("“{}”, {:?} s", metadata.title, metadata.duration)),
                Err(error) => Err(format!("{error:?}")),
            }
        }
        _ => Err("yt-dlp missing".into()),
    };
    checks.record("look up link", started, lookup);

    let started = Instant::now();
    let url = url_validator::validate(&tone_url).expect("a loopback link is valid");
    let job = Job::new(JobSource::Remote(url), OutputSpec::Audio(AudioOptions::default()), "Self Test Download");
    let id = job.id;
    state.queue.enqueue(vec![job]);
    let result = describe(wait_for_job(state, id, Duration::from_secs(180)).await).and_then(|job| {
        let path = job.output_path.unwrap_or_default();
        if path.is_file() { Ok(format!("{} bytes", job.output_size.unwrap_or(0))) } else { Err("no file was delivered".into()) }
    });
    checks.record("download audio", started, result);

    // An image link: looked up and downloaded by MediaForge itself, saved as JPG.
    if have_picture {
        let started = Instant::now();
        let url = url_validator::validate(&server.url("Self%20Test%20Picture.webp")).expect("a loopback link is valid");
        let lookup = match image_link::probe(&url, &CancellationToken::new()).await {
            Ok(Probe::Image(link)) if link.kind == SourceKind::Webp && link.width == Some(64) && link.height == Some(32) => {
                Ok(format!("“{}”, {} {}×{}", link.title, link.kind.name(), 64, 32))
            }
            Ok(other) => Err(format!("unexpected answer: {other:?}")),
            Err(error) => Err(format!("{error:?}")),
        };
        checks.record("look up image link", started, lookup);

        let started = Instant::now();
        let job = Job::new(JobSource::Remote(url.clone()), OutputSpec::Image(ImageOptions::default()), url.display_name());
        let id = job.id;
        state.queue.enqueue(vec![job]);
        let result = describe(wait_for_job(state, id, Duration::from_secs(60)).await).and_then(|job| check_picture(&job, ImageFormat::Jpg));
        checks.record("download image", started, result);
    }

    // Video: a VP9 clip, downloaded and converted to H.264 with this computer's encoder.
    let ffmpeg = state.tools.path(Tool::Ffmpeg);
    let clip = scratch.join("clip.webm");
    let generated = match &ffmpeg {
        Some(ffmpeg) => generate_vp9(ffmpeg, &clip).await,
        None => false,
    };
    if !generated {
        checks.skip(
            "download video",
            "this FFmpeg can't generate a VP9 test clip (no lavfi/libvpx); covered by the macOS integration tests instead",
        );
    } else {
        let started = Instant::now();
        let chosen = state.queue_encoder_choice(VideoCompatibility::H264).await;
        let url = url_validator::validate(&server.url("clip.webm")).expect("a loopback link is valid");
        let job = Job::new(JobSource::Remote(url), OutputSpec::Video(VideoOptions { quality: VideoQuality::Best }), "Self Test Video");
        let id = job.id;
        state.queue.enqueue(vec![job]);
        let result = match describe(wait_for_job(state, id, Duration::from_secs(300)).await) {
            Ok(job) => {
                let path = job.output_path.unwrap_or_default();
                match probe_codecs(state, &path).await {
                    Ok((_, Some(video), height)) if video == "h264" => Ok(format!("VP9 → H.264 {}p with {chosen:?}", height.unwrap_or(0))),
                    Ok((_, video, _)) => Err(format!("expected h264, found {video:?} (encoder {chosen:?})")),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(format!("{error} (encoder {chosen:?})")),
        };
        checks.record("download video", started, result);

        let started = Instant::now();
        let frame = state.artwork.local(&clip, ffmpeg.as_deref()).await;
        checks.record(
            "thumbnail",
            started,
            match frame {
                Some(url) if url.starts_with("data:image/jpeg") => Ok(format!("{} bytes", url.len())),
                _ => Err("no frame extracted".into()),
            },
        );
    }

    // Cancellation part-way through a slow download leaves nothing behind.
    let big = scratch.join("Slow Tone.wav");
    let _ = std::fs::write(&big, local_server::tone_wav(60.0, 44_100, 220.0));
    if let Ok(slow) = LocalServer::start(&scratch, Some(400_000)) {
        let started = Instant::now();
        let url = url_validator::validate(&slow.url("Slow%20Tone.wav")).expect("a loopback link is valid");
        let job = Job::new(JobSource::Remote(url), OutputSpec::Audio(AudioOptions::default()), "Self Test Cancel");
        let id = job.id;
        state.queue.enqueue(vec![job]);
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline && state.queue.job(id).is_some_and(|j| j.state != JobState::Downloading) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        state.queue.cancel(id);
        let job = wait_for_job(state, id, Duration::from_secs(30)).await;
        let leftovers = std::fs::read_dir(&state.paths.work_root).map(|d| d.count()).unwrap_or(0);
        let delivered = state.destination().join("Slow Tone.mp3").exists();
        checks.record(
            "cancel",
            started,
            match job.map(|j| j.state) {
                Some(JobState::Cancelled) if leftovers == 0 && !delivered => Ok("stopped, nothing left behind".into()),
                Some(other) => Err(format!("state {other:?}, {leftovers} working folders left, delivered: {delivered}")),
                None => Err("the job disappeared".into()),
            },
        );
    }

    // Licensing is present but inactive: nothing withheld.
    let summary = state.licensing.summary();
    checks.record(
        "licensing",
        Instant::now(),
        if summary.available_features.len() == crate::licensing::AppFeature::ALL.len() {
            Ok("inactive; every feature available".into())
        } else {
            Err("a feature was withheld".into())
        },
    );
    let _ = app;
}

/// A 64×32 lossless WebP: transparent on the left, red on the right.
fn write_test_webp(path: &Path) -> bool {
    use image::ImageEncoder;
    let picture =
        image::RgbaImage::from_fn(64, 32, |x, _| if x < 32 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([220, 30, 40, 255]) });
    let Ok(file) = std::fs::File::create(path) else { return false };
    image::codecs::webp::WebPEncoder::new_lossless(file).write_image(picture.as_raw(), 64, 32, image::ExtendedColorType::Rgba8).is_ok()
}

/// Checks a converted test picture: the right format and size, and the transparent half white in a JPG or
/// still transparent in a PNG.
fn check_picture(job: &Job, format: ImageFormat) -> Result<String, String> {
    let path = job.output_path.clone().unwrap_or_default();
    let kind = images::identify(&path).map_err(|e| e.message())?;
    let expected = if format == ImageFormat::Jpg { SourceKind::Jpeg } else { SourceKind::Png };
    if kind != expected {
        return Err(format!("expected {}, found {}", expected.name(), kind.name()));
    }
    let decoded = image::ImageReader::open(&path).map_err(|e| e.to_string())?.decode().map_err(|e| e.to_string())?.into_rgba8();
    if decoded.dimensions() != (64, 32) {
        return Err(format!("size {:?}", decoded.dimensions()));
    }
    let corner = decoded.get_pixel(4, 4).0;
    let transparent_ok = match format {
        ImageFormat::Jpg => corner[0] > 250 && corner[1] > 250 && corner[2] > 250,
        ImageFormat::Png => corner[3] == 0,
    };
    if !transparent_ok {
        return Err(format!("transparent area came out as {corner:?}"));
    }
    Ok(format!(
        "{} ({} bytes, {})",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        job.output_size.unwrap_or(0),
        kind.name()
    ))
}

/// A three-second VP9 + Opus clip at 1280×720, like the 4K-only formats large sites serve.
async fn generate_vp9(ffmpeg: &Path, output: &Path) -> bool {
    let arguments: Vec<std::ffi::OsString> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x720:rate=30:duration=3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
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
    ]
    .iter()
    .map(Into::into)
    .chain(std::iter::once(output.as_os_str().to_owned()))
    .collect();
    let mut request = ProcessRequest::new(ffmpeg, arguments);
    request.timeout = Some(Duration::from_secs(120));
    request.working_directory = output.parent().map(Path::to_path_buf);
    matches!(process::run_collect(&request, &CancellationToken::new()).await, Ok(out) if out.exit.success()) && output.is_file()
}

impl AppState {
    async fn queue_encoder_choice(&self, target: VideoCompatibility) -> Option<crate::media::video::EncoderFamily> {
        let ffmpeg = self.tools.path(Tool::Ffmpeg)?;
        crate::media::video::EncoderCache::default().choose(&ffmpeg, target, &CancellationToken::new()).await
    }
}
