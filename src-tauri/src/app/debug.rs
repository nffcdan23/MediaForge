//! Development-only automation, compiled out of release builds.
//!
//! Lets a debug build be driven and inspected from the command line with environment variables:
//!
//! - `MEDIAFORGE_DATA_DIR`, `MEDIAFORGE_CACHE_DIR`, `MEDIAFORGE_DESTINATION`: scratch folders (see `app::setup`).
//! - `MEDIAFORGE_DEBUG_BUNDLED_TOOLS_ONLY=1`: ignore installed tools, to prove a packaged build stands alone.
//! - `MEDIAFORGE_DEBUG_URL`: text to place in the link field after launch (`\n` for several lines).
//! - `MEDIAFORGE_DEBUG_ENQUEUE=1`: press Download once that link has resolved.
//! - `MEDIAFORGE_DEBUG_FILES`: newline-separated paths to add to the conversion queue after launch.
//! - `MEDIAFORGE_DEBUG_SECTION`: downloads, conversions, history or settings.
//! - `MEDIAFORGE_DEBUG_WINDOW_SIZE`: `WIDTHxHEIGHT` in points.
//! - `MEDIAFORGE_DEBUG_SCROLL_TO_END=1`: scroll the current screen to its bottom.
//! - `MEDIAFORGE_DEBUG_ACTION`: a UI action to perform (`terms`, `log:<n>`, `previewMuted`, `videoPreview`,
//!   `imagePreview`), at `MEDIAFORGE_DEBUG_ACTION_AT` seconds after the window is ready.
//! - `MEDIAFORGE_DEBUG_SNAPSHOT`: path prefix for PNGs of the window (macOS).
//! - `MEDIAFORGE_DEBUG_SNAPSHOT_AT`: comma-separated seconds after launch at which to take them.
//! - `MEDIAFORGE_DEBUG_REPORT`: seconds after launch at which to print queue, tools and licensing state.
//! - `MEDIAFORGE_DEBUG_QUIT_AFTER`: seconds after launch at which to quit.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use super::state::{AppState, events};
use crate::models::Section;

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

async fn after(seconds: f64) {
    tokio::time::sleep(Duration::from_secs_f64(seconds.max(0.0))).await;
}

async fn when_ready(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    for _ in 0..300 {
        if state.frontend_ready.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    after(0.5).await;
}

pub fn install(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if super::background_launch() {
        // A window kept behind others counts as hidden to WebKit, which then stops drawing it. Test runs need
        // it drawn for snapshots, so occlusion detection is switched off for them (development builds only).
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.with_webview(|webview| unsafe {
                let view = webview.inner() as *mut objc2::runtime::AnyObject;
                let _: () = objc2::msg_send![view, _setWindowOcclusionDetectionEnabled: false];
            });
        }
    }

    if let Some(size) = var("MEDIAFORGE_DEBUG_WINDOW_SIZE")
        && let Some((w, h)) = size.split_once('x').and_then(|(w, h)| Some((w.parse::<f64>().ok()?, h.parse::<f64>().ok()?)))
        && let Some(window) = app.get_webview_window("main")
    {
        let _ = window.set_size(tauri::LogicalSize::new(w, h));
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        when_ready(&handle).await;
        let state = handle.state::<Arc<AppState>>().inner().clone();
        if let Some(section) = var("MEDIAFORGE_DEBUG_SECTION").and_then(|s| serde_json::from_value::<Section>(json!(s)).ok()) {
            state.navigate(section);
        }
        if let Some(files) = var("MEDIAFORGE_DEBUG_FILES") {
            let paths: Vec<PathBuf> = files.split('\n').map(PathBuf::from).collect();
            if let Err(error) = super::commands::import_paths(&state, &paths, None) {
                eprintln!("DEBUG import failed: {} {}", error.title, error.message);
            }
        }
        if let Some(text) = var("MEDIAFORGE_DEBUG_URL") {
            let enqueue = var("MEDIAFORGE_DEBUG_ENQUEUE").as_deref() == Some("1");
            let _ = handle.emit(events::DEBUG, json!({"action": "setUrl", "value": text.replace("\\n", "\n"), "enqueue": enqueue}));
        }
        if var("MEDIAFORGE_DEBUG_SCROLL_TO_END").as_deref() == Some("1") {
            after(1.0).await;
            let _ = handle.emit(events::DEBUG, json!({"action": "scrollToEnd"}));
        }
        if let Some(action) = var("MEDIAFORGE_DEBUG_ACTION") {
            let delay = var("MEDIAFORGE_DEBUG_ACTION_AT").and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            after(delay).await;
            let _ = handle.emit(events::DEBUG, json!({"action": action}));
        }
    });

    if let Some(prefix) = var("MEDIAFORGE_DEBUG_SNAPSHOT") {
        let times: Vec<f64> =
            var("MEDIAFORGE_DEBUG_SNAPSHOT_AT").unwrap_or_else(|| "3".into()).split(',').filter_map(|t| t.trim().parse().ok()).collect();
        for (index, seconds) in times.into_iter().enumerate() {
            let handle = app.clone();
            let path = PathBuf::from(format!("{prefix}-{}.png", index + 1));
            tauri::async_runtime::spawn(async move {
                after(seconds).await;
                let window = handle.get_webview_window("main");
                let inner = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let Some(window) = window {
                        match snapshot(&window, &path) {
                            Ok(()) => println!("SNAPSHOT {}", path.display()),
                            Err(error) => eprintln!("SNAPSHOT failed: {error}"),
                        }
                    }
                    let _ = inner;
                });
            });
        }
    }

    if let Some(seconds) = var("MEDIAFORGE_DEBUG_REPORT").and_then(|s| s.parse::<f64>().ok()) {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            after(seconds).await;
            let state = handle.state::<Arc<AppState>>().inner().clone();
            for job in state.queue.snapshots() {
                println!(
                    "JOB {:?} state={} title={} output={}",
                    job.kind,
                    serde_json::to_string(&job.state).unwrap_or_default(),
                    job.title,
                    job.output_path.unwrap_or_else(|| "-".into())
                );
            }
            for tool in state.tools.statuses() {
                println!(
                    "TOOL {} bundled={} version={} path={}",
                    tool.name,
                    tool.bundled,
                    tool.version.unwrap_or_else(|| "-".into()),
                    tool.path.unwrap_or_else(|| "MISSING".into())
                );
            }
            println!("SETTINGS {}", serde_json::to_string(&state.settings()).unwrap_or_default());
            println!("HISTORY entries={}", state.history.lock().unwrap().entries().len());
            println!("FRONTEND ready={}", state.frontend_ready.load(std::sync::atomic::Ordering::SeqCst));
            println!("LICENSING {}", serde_json::to_string(&state.licensing.summary()).unwrap_or_default());
        });
    }

    if let Some(seconds) = var("MEDIAFORGE_DEBUG_QUIT_AFTER").and_then(|s| s.parse::<f64>().ok()) {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            after(seconds).await;
            super::begin_quit(&handle);
        });
    }
}

/// Captures the window exactly as the window server composites it.
///
/// An app may always image its own windows, so this needs no Screen Recording permission. The modern
/// ScreenCaptureKit route does, which is why this development aid looks up the older call at runtime.
#[cfg(target_os = "macos")]
fn snapshot(window: &tauri::WebviewWindow, path: &std::path::Path) -> Result<(), String> {
    use std::ffi::{CString, c_void};
    use std::os::unix::ffi::OsStrExt;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }
    type CreateImage = unsafe extern "C" fn(CGRect, u32, u32, u32) -> *const c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: *const c_void,
            buffer: *const u8,
            length: isize,
            is_directory: u8,
        ) -> *const c_void;
        fn CFStringCreateWithCString(allocator: *const c_void, string: *const i8, encoding: u32) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithURL(url: *const c_void, kind: *const c_void, count: usize, options: *const c_void) -> *const c_void;
        fn CGImageDestinationAddImage(destination: *const c_void, image: *const c_void, properties: *const c_void);
        fn CGImageDestinationFinalize(destination: *const c_void) -> u8;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageRelease(image: *const c_void);
    }

    let ns_window = window.ns_window().map_err(|e| e.to_string())? as *mut objc2::runtime::AnyObject;
    let number: isize = unsafe { objc2::msg_send![ns_window, windowNumber] };
    let symbol = CString::new("CGWindowListCreateImage").unwrap();
    let create = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()) };
    if create.is_null() {
        return Err("CGWindowListCreateImage is unavailable".into());
    }
    let create: CreateImage = unsafe { std::mem::transmute(create) };
    let null_rect = CGRect { origin: CGPoint { x: f64::INFINITY, y: f64::INFINITY }, size: CGSize { width: 0.0, height: 0.0 } };
    const INCLUDING_WINDOW: u32 = 1 << 3;
    const BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
    let image = unsafe { create(null_rect, INCLUDING_WINDOW, number as u32, BOUNDS_IGNORE_FRAMING) };
    if image.is_null() {
        return Err("the window couldn't be imaged".into());
    }
    let bytes = path.as_os_str().as_bytes();
    let png = CString::new("public.png").unwrap();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 0);
        let kind = CFStringCreateWithCString(std::ptr::null(), png.as_ptr(), 0x0800_0100);
        let destination = CGImageDestinationCreateWithURL(url, kind, 1, std::ptr::null());
        let ok = !destination.is_null() && {
            CGImageDestinationAddImage(destination, image, std::ptr::null());
            CGImageDestinationFinalize(destination) != 0
        };
        if !destination.is_null() {
            CFRelease(destination);
        }
        CFRelease(kind);
        CFRelease(url);
        CGImageRelease(image);
        if ok { Ok(()) } else { Err("the PNG couldn't be written".into()) }
    }
}

#[cfg(not(target_os = "macos"))]
fn snapshot(_window: &tauri::WebviewWindow, _path: &std::path::Path) -> Result<(), String> {
    Err("window snapshots are only implemented on macOS".into())
}
