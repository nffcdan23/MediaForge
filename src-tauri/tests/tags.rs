//! End-to-end tests of the Metadata screen's engine with real audio and video files, made on the spot by
//! FFmpeg and checked afterwards by ffprobe and FFmpeg itself. Tests that need FFmpeg pass without running
//! when it isn't installed (see `engine.rs` for where tools are looked for).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mediaforge_lib::process::{self, ProcessRequest};
use mediaforge_lib::tags::{self, Change, CoverChange, Field, Tools};
use mediaforge_lib::tools::{self, BundledTools, Tool, ToolLocator};
use tokio_util::sync::CancellationToken;

fn tools() -> Tools {
    let given: Vec<PathBuf> = std::env::var_os("MEDIAFORGE_TEST_TOOLS").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    let mut search = given;
    search.extend(tools::default_search_directories());
    let locator = ToolLocator { search_directories: search, overrides: Default::default(), bundled: BundledTools::default() };
    Tools { ffmpeg: locator.locate(Tool::Ffmpeg).ok(), ffprobe: locator.locate(Tool::Ffprobe).ok() }
}

macro_rules! require_ffmpeg {
    () => {{
        let tools = tools();
        if tools.ffmpeg.is_none() || tools.ffprobe.is_none() {
            eprintln!("skipped: FFmpeg not available");
            return;
        }
        tools
    }};
}

async fn run(executable: &Path, args: &[&str]) -> (bool, Vec<u8>, String) {
    let mut request = ProcessRequest::new(executable, args.iter().map(Into::into).collect());
    request.timeout = Some(Duration::from_secs(120));
    let output = process::run_collect(&request, &CancellationToken::new()).await.expect("tool runs");
    (output.exit.success(), output.stdout, output.stderr)
}

/// Makes a media file with FFmpeg from generated sound (and pictures, for video).
async fn make(tools: &Tools, output: &Path, args: &[&str]) -> bool {
    let mut all = vec!["-hide_banner", "-loglevel", "error", "-y"];
    all.extend_from_slice(args);
    let output = output.to_string_lossy().into_owned();
    all.push(&output);
    run(tools.ffmpeg.as_ref().unwrap(), &all).await.0
}

/// Format-level tags as ffprobe sees them, keys in lower case.
async fn probe_tags(tools: &Tools, path: &Path) -> (BTreeMap<String, String>, serde_json::Value) {
    let path = path.to_string_lossy().into_owned();
    let (ok, stdout, stderr) =
        run(tools.ffprobe.as_ref().unwrap(), &["-v", "error", "-print_format", "json", "-show_format", "-show_streams", &path]).await;
    assert!(ok, "ffprobe failed: {stderr}");
    let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    // Ogg files keep their tags on the stream rather than the file.
    let audio_stream = json["streams"].as_array().and_then(|s| s.iter().find(|s| s["codec_type"] == "audio")).cloned().unwrap_or_default();
    for source in [&audio_stream["tags"], &json["format"]["tags"]] {
        if let Some(object) = source.as_object() {
            tags.extend(object.iter().map(|(k, v)| (k.to_lowercase(), v.as_str().unwrap_or_default().to_string())));
        }
    }
    (tags, json)
}

/// The whole file still decodes without a single error.
async fn decodes_cleanly(tools: &Tools, path: &Path) {
    let path = path.to_string_lossy().into_owned();
    let (ok, _, stderr) = run(tools.ffmpeg.as_ref().unwrap(), &["-v", "error", "-i", &path, "-f", "null", "-"]).await;
    assert!(ok && stderr.trim().is_empty(), "decoding {path} failed: {stderr}");
}

fn cover() -> tags::CoverImage {
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 90])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    tags::prepare_cover(&png).unwrap()
}

fn music_fields() -> BTreeMap<Field, String> {
    BTreeMap::from([
        (Field::Title, "Night Drive".to_string()),
        (Field::Artist, "Zoë & The Loops".to_string()),
        (Field::Album, "Unreleased Demos".to_string()),
        (Field::AlbumArtist, "Various Artists".to_string()),
        (Field::Track, "3".to_string()),
        (Field::TrackTotal, "12".to_string()),
        (Field::Disc, "1".to_string()),
        (Field::Year, "2021".to_string()),
        (Field::Genre, "Synthwave".to_string()),
        (Field::Composer, "A. Writer".to_string()),
        (Field::Comment, "Ripped from my own master".to_string()),
    ])
}

#[tokio::test]
async fn mp3s_get_id3v2_3_tags_and_artwork_that_spotify_and_ffmpeg_read() {
    let tools = require_ffmpeg!();
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("track.mp3");
    assert!(
        make(
            &tools,
            &path,
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "128k",
                "-id3v2_version",
                "4",
                "-write_id3v1",
                "1",
                "-metadata",
                "title=Old title",
                "-metadata",
                "artist=Old artist",
                "-metadata",
                "lyrics=Keep these words"
            ],
        )
        .await
    );
    let before = tags::read(&path, &tools).await.unwrap();
    assert_eq!(before.fields[&Field::Title], "Old title");
    assert_eq!(before.storage.as_deref(), Some("ID3v2.4 · ID3v1"));

    let change = Change { fields: music_fields(), cover: Some(CoverChange::Set(cover())), ..Default::default() };
    tags::save(&path, change, &tools, false).await.unwrap();

    let after = tags::read(&path, &tools).await.unwrap();
    for (field, value) in music_fields() {
        assert_eq!(after.fields.get(&field), Some(&value), "{field:?}");
    }
    assert_eq!(after.storage.as_deref(), Some("ID3v2.3 · ID3v1"));
    assert!(after.cover.as_ref().unwrap().present);
    assert_eq!(&std::fs::read(&path).unwrap()[..4], b"ID3\x03");

    let (probed, json) = probe_tags(&tools, &path).await;
    assert_eq!(probed["title"], "Night Drive");
    assert_eq!(probed["artist"], "Zoë & The Loops");
    assert_eq!(probed["album_artist"], "Various Artists");
    assert_eq!(probed["track"], "3/12");
    assert!(probed.get("date").or(probed.get("tyer")).is_some_and(|d| d.starts_with("2021")), "{probed:?}");
    // Frames MediaForge doesn't show, such as lyrics, are kept.
    assert!(probed.iter().any(|(k, v)| k.starts_with("lyrics") && v == "Keep these words"), "{probed:?}");
    let attached = json["streams"].as_array().unwrap().iter().any(|s| s["disposition"]["attached_pic"] == 1);
    assert!(attached, "the artwork is an attached picture");
    decodes_cleanly(&tools, &path).await;
    assert!(tags::cover_data(&path).is_some());

    // Clearing fields and artwork removes them.
    let change = Change {
        fields: BTreeMap::from([(Field::Comment, String::new()), (Field::Track, String::new())]),
        cover: Some(CoverChange::Remove),
        ..Default::default()
    };
    tags::save(&path, change, &tools, false).await.unwrap();
    let cleared = tags::read(&path, &tools).await.unwrap();
    assert!(!cleared.fields.contains_key(&Field::Comment));
    assert!(!cleared.fields.contains_key(&Field::Track));
    assert!(!cleared.cover.unwrap().present);
    assert_eq!(cleared.fields[&Field::Title], "Night Drive");
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1, "no temporary files are left behind");
}

#[tokio::test]
async fn other_audio_formats_round_trip() {
    let tools = require_ffmpeg!();
    let folder = tempfile::tempdir().unwrap();
    for (name, codec) in [
        ("song.m4a", "aac"),
        ("song.flac", "flac"),
        ("song.wav", "pcm_s16le"),
        ("song.ogg", "libvorbis"),
        ("song.opus", "libopus"),
        ("song.aiff", "pcm_s16be"),
    ] {
        let path = folder.path().join(name);
        if !make(&tools, &path, &["-f", "lavfi", "-i", "sine=frequency=330:duration=1", "-c:a", codec]).await {
            eprintln!("skipped {name}: this FFmpeg has no {codec} encoder");
            continue;
        }
        let change = Change { fields: music_fields(), cover: Some(CoverChange::Set(cover())), ..Default::default() };
        tags::save(&path, change, &tools, false).await.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let after = tags::read(&path, &tools).await.unwrap();
        assert_eq!(after.fields[&Field::Title], "Night Drive", "{name}");
        assert_eq!(after.fields[&Field::Artist], "Zoë & The Loops", "{name}");
        assert_eq!(after.fields[&Field::Track], "3", "{name}");
        assert!(after.cover.unwrap().present, "{name}");
        let (probed, _) = probe_tags(&tools, &path).await;
        assert_eq!(probed.get("title").map(String::as_str), Some("Night Drive"), "{name}: {probed:?}");
        decodes_cleanly(&tools, &path).await;
    }
}

#[tokio::test]
async fn mp4_video_keeps_playing_after_its_tags_change() {
    let tools = require_ffmpeg!();
    let folder = tempfile::tempdir().unwrap();
    for (name, extra) in [("clip.mp4", vec!["-movflags", "+faststart"]), ("clip.mov", vec![]), ("late-moov.mp4", vec![])] {
        let path = folder.path().join(name);
        let mut args = vec![
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=320x240:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=2",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-shortest",
        ];
        args.extend(extra);
        assert!(make(&tools, &path, &args).await, "{name}");
        let fields = BTreeMap::from([
            (Field::Title, "Holiday 2026".to_string()),
            (Field::Comment, "Day one".to_string()),
            (Field::Description, "A long description of the clip".to_string()),
            (Field::Year, "2026".to_string()),
        ]);
        let change = Change { fields, cover: Some(CoverChange::Set(cover())), ..Default::default() };
        tags::save(&path, change, &tools, false).await.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let after = tags::read(&path, &tools).await.unwrap();
        assert_eq!(after.kind, tags::FileKind::Video);
        assert_eq!(after.fields[&Field::Title], "Holiday 2026", "{name}");
        let (probed, json) = probe_tags(&tools, &path).await;
        assert_eq!(probed.get("title").map(String::as_str), Some("Holiday 2026"), "{name}");
        assert!(json["streams"].as_array().unwrap().iter().any(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] == 0));
        decodes_cleanly(&tools, &path).await;
    }
}

#[tokio::test]
async fn matroska_webm_and_avi_tags_are_rewritten_by_ffmpeg() {
    let tools = require_ffmpeg!();
    let folder = tempfile::tempdir().unwrap();
    for (name, video) in [("film.mkv", "mpeg4"), ("film.avi", "mpeg4"), ("film.webm", "libvpx")] {
        let path = folder.path().join(name);
        // The audio real files of each kind carry. (AAC in AVI is non-standard, and FFmpeg can't copy it cleanly.)
        let audio = match name {
            "film.webm" => "libopus",
            "film.avi" => "libmp3lame",
            _ => "aac",
        };
        if !make(
            &tools,
            &path,
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=160x120:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=1",
                "-c:v",
                video,
                "-c:a",
                audio,
                "-shortest",
                "-metadata",
                "comment=Old note",
            ],
        )
        .await
        {
            eprintln!("skipped {name}: encoders missing");
            continue;
        }
        let before = tags::read(&path, &tools).await.unwrap();
        assert!(before.editable.contains(&Field::Title), "{name}");
        let fields = BTreeMap::from([
            (Field::Title, "Final cut".to_string()),
            (Field::Artist, "Studio".to_string()),
            (Field::Comment, String::new()),
        ]);
        tags::save(&path, Change { fields, ..Default::default() }, &tools, false).await.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let after = tags::read(&path, &tools).await.unwrap();
        assert_eq!(after.fields.get(&Field::Title).map(String::as_str), Some("Final cut"), "{name}");
        assert_eq!(after.fields.get(&Field::Artist).map(String::as_str), Some("Studio"), "{name}");
        assert!(!after.fields.contains_key(&Field::Comment), "{name}: {:?}", after.fields);
        decodes_cleanly(&tools, &path).await;
        assert_eq!(
            std::fs::read_dir(folder.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("~$"))
                .count(),
            0
        );
    }
}

#[tokio::test]
async fn any_file_can_be_renamed_redated_and_made_read_only() {
    let tools = tools();
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("notes.txt");
    std::fs::write(&path, "hello").unwrap();
    let file = tags::read(&path, &tools).await.unwrap();
    assert_eq!((file.kind, file.editable.len()), (tags::FileKind::Document, 0));

    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    let change = Change { name: Some("Meeting notes".into()), modified: Some(when), read_only: Some(true), ..Default::default() };
    let moved = tags::save(&path, change, &tools, false).await.unwrap();
    assert_eq!(moved, folder.path().join("Meeting notes.txt"));
    let metadata = std::fs::metadata(&moved).unwrap();
    assert_eq!(metadata.modified().unwrap(), when);
    assert!(metadata.permissions().readonly());

    // Read-only files refuse new dates until Read-only is turned off, which can happen in the same save.
    let blocked = tags::save(&moved, Change { modified: Some(SystemTime::now()), ..Default::default() }, &tools, false).await;
    assert!(blocked.is_err());
    tags::save(
        &moved,
        Change { read_only: Some(false), modified: Some(when + Duration::from_secs(60)), ..Default::default() },
        &tools,
        false,
    )
    .await
    .unwrap();
    assert!(!std::fs::metadata(&moved).unwrap().permissions().readonly());
}

#[tokio::test]
async fn keeping_dates_leaves_the_modified_date_alone() {
    let tools = require_ffmpeg!();
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("keep.mp3");
    assert!(make(&tools, &path, &["-f", "lavfi", "-i", "sine=duration=1", "-c:a", "libmp3lame"]).await);
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
    tags::save(&path, Change { modified: Some(when), ..Default::default() }, &tools, false).await.unwrap();
    tags::save(&path, Change { fields: BTreeMap::from([(Field::Title, "Kept".into())]), ..Default::default() }, &tools, true)
        .await
        .unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), when);
    tags::save(&path, Change { fields: BTreeMap::from([(Field::Title, "Moved on".into())]), ..Default::default() }, &tools, false)
        .await
        .unwrap();
    assert_ne!(std::fs::metadata(&path).unwrap().modified().unwrap(), when);
}
