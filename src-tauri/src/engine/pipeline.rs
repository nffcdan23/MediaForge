//! Runs one job from start to finish: download, convert, then move the result into place.
//!
//! Audio and video are fetched by yt-dlp and converted by FFmpeg. Images are fetched by MediaForge itself
//! (media::image_link) and converted in-process (media::image).
//!
//! Work happens in a private temporary folder. Nothing appears in the destination until it is complete, and
//! the folder is removed however the job ends.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::engine::job::{Job, JobProgress, JobSource, JobState};
use crate::engine::output_files;
use crate::engine::queue::{JobEnvironment, JobHandle, removed};
use crate::error::{Failure, JobResult, MediaForgeError};
use crate::media::ffmpeg::{self, Ffmpeg, MediaTags, ProgressParser};
use crate::media::image::{self as images, Sniffed};
use crate::media::image_link;
use crate::media::metadata::MediaMetadata;
use crate::media::video::{self, EncoderFamily};
use crate::media::ytdlp::{self, ProgressAggregator, YtDlp, YtDlpEvent};
use crate::models::{AudioOptions, ImageOptions, OutputSpec};
use crate::process::{ProcessEvent, ProcessExit};
use crate::support::url_validator::ValidatedUrl;
use crate::tools::Tool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub path: PathBuf,
    pub size: u64,
}

pub async fn run(job: &JobHandle, environment: &JobEnvironment, cancel: &CancellationToken) -> JobResult<Delivered> {
    let attempt = job.read(|j| j.attempt).ok_or_else(removed)?;
    let work = output_files::make_work_directory(&environment.work_root, job.id(), attempt)?;
    let result = perform(job, environment, cancel, &work).await;
    output_files::remove_work_directory(&work);
    result
}

async fn perform(job: &JobHandle, environment: &JobEnvironment, cancel: &CancellationToken, work: &Path) -> JobResult<Delivered> {
    job.set_state(JobState::Preparing);
    let destination = environment.destination.clone();
    tokio::task::spawn_blocking(move || output_files::prepare_destination(&destination))
        .await
        .map_err(|e| MediaForgeError::Unknown(e.to_string()))??;

    let (source, mut output, title, detail) =
        job.read(|j| (j.source.clone(), j.output, j.title.clone(), j.detail.clone())).ok_or_else(removed)?;
    // An image link in a batch queued for audio or video: it can only become an image.
    if let JobSource::Remote(url) = &source
        && !matches!(output, OutputSpec::Image(_))
        && image_link::looks_like_image(url)
    {
        output = as_image(job, environment);
    }
    let finished = match (&source, output) {
        (JobSource::Remote(url), OutputSpec::Image(options)) => {
            let downloaded = download_image(job, environment, cancel, url, work).await?;
            convert_image(job, &downloaded, options, work).await?
        }
        (JobSource::Remote(url), media) => {
            let downloaded = download(job, environment, cancel, url, &media, work).await?;
            // A link with no file name can still turn out to be a picture: yt-dlp fetches any file that isn't
            // a web page.
            if let Ok(head) = read_head(&downloaded)
                && let Sniffed::Image(_) = images::sniff(&head)
            {
                output = as_image(job, environment);
                convert_image(job, &downloaded, environment.image_options, work).await?
            } else if let OutputSpec::Audio(options) = media {
                let tags = if environment.embed_metadata {
                    let (title, detail) = job.read(|j| (j.title.clone(), j.detail.clone())).unwrap_or((title, detail));
                    MediaTags { title: Some(title), artist: detail, comment: Some(url.as_str().to_string()) }
                } else {
                    MediaTags::default()
                };
                convert(job, environment, cancel, &downloaded, &options, &tags, work).await?
            } else {
                make_compatible(job, environment, cancel, downloaded, work).await?
            }
        }
        (JobSource::File(file), OutputSpec::Audio(options)) => {
            convert(job, environment, cancel, file, &options, &MediaTags::default(), work).await?
        }
        (JobSource::File(file), OutputSpec::Image(options)) => convert_image(job, file, options, work).await?,
        (JobSource::File(_), OutputSpec::Video(_)) => return Err(MediaForgeError::NoMatchingFormat.into()),
    };

    check(cancel)?;
    job.set_state(JobState::Finalizing);
    let title = job.read(|j| j.title.clone()).ok_or_else(removed)?;
    let destination = environment.destination.clone();
    let extension = output.extension().to_string();
    let placed = tokio::task::spawn_blocking(move || output_files::move_into_place(&finished, &destination, &title, &extension))
        .await
        .map_err(|e| MediaForgeError::Unknown(e.to_string()))??;
    if let JobSource::Remote(url) = &source {
        output_files::mark_as_downloaded(&placed.path, url.as_str());
    }
    Ok(Delivered { path: placed.path, size: placed.size })
}

fn check(cancel: &CancellationToken) -> JobResult<()> {
    if cancel.is_cancelled() { Err(Failure::Cancelled) } else { Ok(()) }
}

fn size_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn as_image(job: &JobHandle, environment: &JobEnvironment) -> OutputSpec {
    let output = OutputSpec::Image(environment.image_options);
    job.update(|j| j.output = output);
    output
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::with_capacity(32);
    std::fs::File::open(path)?.take(32).read_to_end(&mut head)?;
    Ok(head)
}

// MARK: - Images

async fn download_image(
    job: &JobHandle,
    environment: &JobEnvironment,
    cancel: &CancellationToken,
    url: &ValidatedUrl,
    work: &Path,
) -> JobResult<PathBuf> {
    let downloaded = image_link::download(url, work, environment.limits, cancel, |transfer| {
        let first = job.read(|j| j.state != JobState::Downloading).unwrap_or(false);
        let change = |j: &mut Job| {
            j.state = JobState::Downloading;
            let remaining = match (transfer.total, transfer.bytes_per_second) {
                (Some(total), Some(speed)) if speed > 0.0 => Some(total.saturating_sub(transfer.received) as f64 / speed),
                _ => None,
            };
            j.progress = JobProgress {
                fraction: transfer.total.map(|total| (transfer.received as f64 / total as f64).min(1.0)),
                bytes_per_second: transfer.bytes_per_second,
                seconds_remaining: remaining,
                downloaded_bytes: Some(transfer.received),
                total_bytes: transfer.total,
                conversion_speed: None,
            };
        };
        if first { job.update(change) } else { job.progress(change) }
    })
    .await?;
    // A link queued without a title (one of a batch) is named after its file.
    if job.read(|j| j.title == url.display_name()).unwrap_or(false) {
        job.update(|j| j.title = downloaded.title.clone());
    }
    Ok(downloaded.path)
}

async fn convert_image(job: &JobHandle, input: &Path, options: ImageOptions, work: &Path) -> JobResult<PathBuf> {
    job.set_state(JobState::Converting);
    let input = input.to_path_buf();
    let output = work.join(format!("converted.{}", options.format.extension()));
    let target = output.clone();
    tokio::task::spawn_blocking(move || images::convert(&input, &target, options))
        .await
        .map_err(|_| MediaForgeError::ConversionFailed("MediaForge couldn't convert this image.".into()))??;
    Ok(output)
}

// MARK: - Download

async fn download(
    job: &JobHandle,
    environment: &JobEnvironment,
    cancel: &CancellationToken,
    url: &ValidatedUrl,
    output: &OutputSpec,
    work: &Path,
) -> JobResult<PathBuf> {
    let ytdlp = environment.toolchain.ytdlp.clone().ok_or(MediaForgeError::ToolMissing(Tool::YtDlp))?;
    let ffmpeg = environment.toolchain.ffmpeg.clone().ok_or(MediaForgeError::ToolMissing(Tool::Ffmpeg))?;
    let service = YtDlp {
        executable: ytdlp,
        ffmpeg_directory: ffmpeg.parent().map(Path::to_path_buf),
        support_directories: environment.toolchain.support_directories.clone(),
    };
    let info_path = work.join(ytdlp::info_json_name());

    let mut aggregator = ProgressAggregator::default();
    let mut selected_formats: Vec<String> = Vec::new();
    let mut metadata: Option<MediaMetadata> = None;
    let mut info_announced = false;
    let mut stderr: Vec<String> = Vec::new();
    let mut hit_size_limit = false;
    let mut skipped_live = false;

    // yt-dlp announces the sidecar just before writing it, so it is read on the following event.
    let load_info = |announced: bool, metadata: &mut Option<MediaMetadata>, aggregator: &mut ProgressAggregator, formats: &[String]| {
        if !announced || metadata.is_some() {
            return;
        }
        let Ok(loaded) = MediaMetadata::load(&info_path) else { return };
        job.update(|j| j.apply(&loaded));
        aggregator.set_parts(&sizes(formats, &loaded));
        *metadata = Some(loaded);
    };

    let exit: ProcessExit = service
        .download(url, output, environment.limits, environment.embed_metadata, environment.video_compatibility, work, cancel, |event| {
            match event {
                ProcessEvent::Stdout(line) => {
                    let Some(parsed) = ytdlp::parse_line(&line) else {
                        job.log(&line);
                        return;
                    };
                    match parsed {
                        YtDlpEvent::SelectedFormats(ids) => {
                            aggregator.set_parts(&vec![None; ids.len()]);
                            selected_formats = ids;
                        }
                        YtDlpEvent::InfoJsonWritten(_) => info_announced = true,
                        YtDlpEvent::Destination(_) => {
                            load_info(info_announced, &mut metadata, &mut aggregator, &selected_formats);
                            aggregator.begin_part();
                            job.update(|j| {
                                if j.state != JobState::Downloading {
                                    j.state = JobState::Downloading;
                                }
                            });
                        }
                        YtDlpEvent::Progress(progress) => {
                            load_info(info_announced, &mut metadata, &mut aggregator, &selected_formats);
                            let fraction = aggregator.overall_fraction(&progress);
                            let first = job.read(|j| j.state != JobState::Downloading).unwrap_or(false);
                            let change = |j: &mut crate::engine::job::Job| {
                                j.state = JobState::Downloading;
                                j.progress = JobProgress {
                                    fraction,
                                    bytes_per_second: progress.bytes_per_second,
                                    seconds_remaining: progress.seconds_remaining,
                                    downloaded_bytes: progress.downloaded_bytes,
                                    total_bytes: progress.total_bytes,
                                    conversion_speed: None,
                                };
                            };
                            if first { job.update(change) } else { job.progress(change) }
                        }
                        YtDlpEvent::PostProcessing(stage) => job.set_state(JobState::Processing(stage.to_string())),
                        YtDlpEvent::SizeLimitExceeded => hit_size_limit = true,
                        YtDlpEvent::SkippedLive => skipped_live = true,
                        YtDlpEvent::AlreadyDownloaded => {}
                    }
                }
                ProcessEvent::Stderr(line) => {
                    if stderr.len() < 400 {
                        stderr.push(line.clone());
                    }
                    job.log(&line);
                }
            }
        })
        .await?;
    check(cancel)?;

    if hit_size_limit {
        return Err(MediaForgeError::ExceedsSizeLimit.into());
    }
    if skipped_live {
        return Err(MediaForgeError::LiveStream.into());
    }
    if !exit.success() {
        return Err(ytdlp::classify(&stderr).into());
    }
    load_info(true, &mut metadata, &mut aggregator, &selected_formats);

    ytdlp::locate_download(work)
        .ok_or_else(|| MediaForgeError::DownloadFailed("The source reported success but didn't send a file.".into()).into())
}

fn sizes(format_ids: &[String], metadata: &MediaMetadata) -> Vec<Option<u64>> {
    if !metadata.requested_formats.is_empty() && (format_ids.is_empty() || metadata.requested_formats.len() == format_ids.len()) {
        return metadata.requested_formats.iter().map(|f| f.file_size).collect();
    }
    format_ids
        .iter()
        .map(|id| metadata.requested_formats.iter().chain(&metadata.formats).find(|f| &f.id == id).and_then(|f| f.file_size))
        .collect()
}

// MARK: - Video

/// Checks a downloaded video really contains video, then, if it's in a format most players and editors can't
/// open and the user has asked for compatible files, re-encodes it. Returns the file to deliver.
async fn make_compatible(
    job: &JobHandle,
    environment: &JobEnvironment,
    cancel: &CancellationToken,
    file: PathBuf,
    work: &Path,
) -> JobResult<PathBuf> {
    let (Some(ffmpeg_path), Some(ffprobe_path)) = (environment.toolchain.ffmpeg.clone(), environment.toolchain.ffprobe.clone()) else {
        return Ok(file);
    };
    let service = Ffmpeg { ffmpeg: ffmpeg_path.clone(), ffprobe: ffprobe_path };
    let probe = match service.probe(&file, cancel).await {
        Ok(probe) => probe,
        Err(Failure::Cancelled) => return Err(Failure::Cancelled),
        Err(_) => return Ok(file),
    };
    check(cancel)?;
    // Guards against a "video" download that turned out to contain only sound.
    if !probe.has_video() {
        return Err(MediaForgeError::NoMatchingFormat.into());
    }
    if job.read(|j| j.duration.is_none()).unwrap_or(false) {
        job.update(|j| j.duration = probe.duration);
    }
    let target = environment.video_compatibility;
    if !target.converts() || !video::needs_conversion(&probe) {
        return Ok(file);
    }

    job.set_state(JobState::Converting);
    let Some(chosen) = job.encoders().choose(&ffmpeg_path, target, cancel).await else {
        check(cancel)?;
        return Err(MediaForgeError::ConversionFailed(
            "No video encoder on this computer could convert this download. Choose “Keep the original format” in Settings ▸ Video to save it as it is.".into(),
        )
        .into());
    };
    let output = work.join("compatible.mp4");
    let mut attempt = chosen;
    loop {
        let arguments = video::arguments(&file, &output, &probe, target, attempt);
        match run_ffmpeg(job, &service, arguments, work, probe.duration, Duration::from_secs(12 * 60 * 60), cancel, true).await {
            Ok(()) => break,
            // A graphics encoder that passed its test can still refuse a real 4K stream; Windows' own encoder
            // is slower but takes anything.
            Err(Failure::Error(_)) if cfg!(windows) && attempt != EncoderFamily::MediaFoundation => {
                let _ = std::fs::remove_file(&output);
                job.log(&format!("{} failed; retrying with Windows' built-in encoder", attempt.encoder(target).unwrap_or("encoder")));
                attempt = EncoderFamily::MediaFoundation;
            }
            Err(error) => return Err(error),
        }
    }
    // The original is no longer needed, and for 4K it is large.
    let _ = std::fs::remove_file(&file);
    Ok(output)
}

// MARK: - Conversion

async fn convert(
    job: &JobHandle,
    environment: &JobEnvironment,
    cancel: &CancellationToken,
    input: &Path,
    options: &AudioOptions,
    tags: &MediaTags,
    work: &Path,
) -> JobResult<PathBuf> {
    let ffmpeg_path = environment.toolchain.ffmpeg.clone().ok_or(MediaForgeError::ToolMissing(Tool::Ffmpeg))?;
    let ffprobe_path = environment.toolchain.ffprobe.clone().ok_or(MediaForgeError::ToolMissing(Tool::Ffprobe))?;
    let service = Ffmpeg { ffmpeg: ffmpeg_path, ffprobe: ffprobe_path };

    job.set_state(JobState::Converting);
    let probe = service.probe(input, cancel).await?;
    if probe.audio.is_none() {
        return Err(MediaForgeError::NoAudioStream.into());
    }
    if job.read(|j| j.duration.is_none()).unwrap_or(false) {
        job.update(|j| j.duration = probe.duration);
    }
    let output = work.join(format!("converted.{}", options.format.extension()));
    let arguments = ffmpeg::conversion_arguments(input, &output, options, Some(&probe), tags);
    run_ffmpeg(job, &service, arguments, work, probe.duration, Duration::from_secs(6 * 60 * 60), cancel, false).await?;
    if size_of(&output) == 0 {
        return Err(MediaForgeError::ConversionFailed(String::new()).into());
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
async fn run_ffmpeg(
    job: &JobHandle,
    service: &Ffmpeg,
    arguments: Vec<std::ffi::OsString>,
    work: &Path,
    duration: Option<f64>,
    timeout: Duration,
    cancel: &CancellationToken,
    is_video: bool,
) -> JobResult<()> {
    let mut parser = ProgressParser::new(duration);
    let mut stderr: Vec<String> = Vec::new();
    let exit = service
        .run(arguments, work, timeout, cancel, |event| match event {
            ProcessEvent::Stdout(line) => {
                if let Some(progress) = parser.consume(&line) {
                    job.progress(|j| {
                        j.progress = JobProgress {
                            fraction: progress.fraction,
                            seconds_remaining: progress.seconds_remaining,
                            conversion_speed: progress.speed,
                            ..Default::default()
                        };
                    });
                }
            }
            ProcessEvent::Stderr(line) => {
                if stderr.len() < 400 {
                    stderr.push(line.clone());
                }
                job.log(&line);
            }
        })
        .await?;
    check(cancel)?;
    if !exit.success() {
        let summary = if is_video { ffmpeg::summarize_video(&stderr) } else { ffmpeg::summarize(&stderr) };
        return Err(MediaForgeError::ConversionFailed(summary).into());
    }
    Ok(())
}
