//! The desktop app: window, menus, events and start-up. Everything platform-facing lives here; the engine in
//! `crate::engine` knows nothing about Tauri.

pub mod commands;
#[cfg(debug_assertions)]
mod debug;
pub mod local_server;
#[cfg(target_os = "macos")]
mod menu;
mod self_test;
pub mod state;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::{Action, ActionType, NotificationExt};
use tauri_plugin_opener::OpenerExt;

use crate::engine::output_files;
use crate::models::{Appearance, Section};
use state::{AppPaths, AppState, NOTIFICATION_ACTION_TYPE, NOTIFICATION_REVEAL, events};

pub fn run() {
    let self_test = self_test::requested();
    let mut builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    if background_launch() || self_test.is_some() {
        // Test runs open behind whatever the user is doing instead of taking the keyboard.
        builder = builder.activate_ignoring_other_apps(false);
    }
    if self_test.is_none() {
        // A second launch (for example "Open with MediaForge" on Windows) hands its files to the running copy.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let Some(state) = app.try_state::<Arc<AppState>>() else { return };
            state.show_window();
            open_files(&state, file_arguments(argv.into_iter().skip(1)));
        }));
    }
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(move |app| {
            setup(app.handle(), self_test.clone())?;
            Ok(())
        })
        .on_window_event(on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::update_settings,
            commands::choose_destination,
            commands::open_destination,
            commands::choose_tool_path,
            commands::reset_tool_path,
            commands::refresh_tools,
            commands::acknowledge_terms,
            commands::set_section,
            commands::quit_app,
            commands::inspect_links,
            commands::resolve_link,
            commands::cancel_resolve,
            commands::enqueue_downloads,
            commands::import_files,
            commands::choose_files_to_convert,
            commands::cancel_job,
            commands::retry_job,
            commands::remove_job,
            commands::cancel_all,
            commands::clear_finished,
            commands::job_log,
            commands::reveal_path,
            commands::open_path,
            commands::copy_text,
            commands::read_clipboard,
            commands::open_third_party_notices,
            commands::remove_history,
            commands::clear_history,
            commands::repeat_history,
            commands::thumbnail,
            commands::waveform,
            commands::preview_source,
            commands::licensing_summary,
            commands::show_main_window,
        ])
        .build(tauri::generate_context!())
        .expect("MediaForge couldn't start");
    app.run(on_run_event);
}

/// Development builds started with `MEDIAFORGE_DEBUG_BACKGROUND=1` stay behind other windows.
pub fn background_launch() -> bool {
    cfg!(debug_assertions) && std::env::var("MEDIAFORGE_DEBUG_BACKGROUND").as_deref() == Ok("1")
}

fn env_path(key: &str) -> Option<PathBuf> {
    // Development builds can be pointed at scratch folders, so testing never touches real settings or files.
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn setup(app: &AppHandle, self_test: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let resolver = app.path();
    let scratch = self_test.as_ref().map(|_| std::env::temp_dir().join(format!("MediaForge-SelfTest-{}", std::process::id())));
    let data =
        scratch.clone().map(|s| s.join("data")).or_else(|| env_path("MEDIAFORGE_DATA_DIR")).unwrap_or(resolver.app_local_data_dir()?);
    let cache = scratch.clone().map(|s| s.join("cache")).or_else(|| env_path("MEDIAFORGE_CACHE_DIR")).unwrap_or(resolver.app_cache_dir()?);
    let downloads =
        resolver.download_dir().ok().or_else(|| resolver.home_dir().ok().map(|h| h.join("Downloads"))).unwrap_or_else(std::env::temp_dir);
    let default_destination =
        scratch.clone().map(|s| s.join("out")).or_else(|| env_path("MEDIAFORGE_DESTINATION")).unwrap_or(downloads.join("MediaForge"));
    let paths = AppPaths {
        data,
        cache,
        default_destination,
        executable_directory: std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())),
        resource_directory: resolver.resource_dir().ok(),
        work_root: output_files::default_work_root(),
    };
    let import_legacy = cfg!(target_os = "macos")
        && self_test.is_none()
        && (!cfg!(debug_assertions) || std::env::var("MEDIAFORGE_IMPORT_LEGACY").as_deref() == Ok("1"));
    let state = AppState::new(app.clone(), paths, import_legacy);
    app.manage(state.clone());

    // Working folders whose owner has gone belong to a run that ended abruptly.
    tauri::async_runtime::spawn_blocking(|| {
        output_files::purge_abandoned_work(
            &output_files::work_container(),
            std::process::id(),
            std::time::SystemTime::now(),
            output_files::process_is_running,
        );
    });
    state.tools.refresh(None);

    // Finished-job notifications offer to show the file, as the macOS app's did.
    let reveal_title = if cfg!(target_os = "macos") { "Show in Finder" } else { "Show in File Explorer" };
    let _ = app.notification().register_action_types(vec![
        ActionType::builder(NOTIFICATION_ACTION_TYPE).actions(vec![Action::builder(NOTIFICATION_REVEAL, reveal_title).build()]).build(),
    ]);
    let action_handle = app.clone();
    let _ = app.notification().on_action(move |performed| {
        let Some(state) = action_handle.try_state::<Arc<AppState>>() else { return };
        state.show_window();
        if performed.action_id() != NOTIFICATION_REVEAL {
            return;
        }
        let path = performed.notification().and_then(|n| n.extra().get("path")).and_then(|v| v.as_str()).map(PathBuf::from);
        if let Some(path) = path.filter(|p| p.is_file() && state.is_known_output(p)) {
            let _ = action_handle.opener().reveal_item_in_dir(path);
        }
    });

    #[cfg(target_os = "macos")]
    menu::install(app)?;
    apply_appearance(app, state.settings().appearance);

    // The window starts hidden and is shown once the UI has drawn, so it never flashes white. A fallback
    // shows it regardless, in case the UI is slow to load.
    let fallback = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        if let Some(window) = fallback.get_webview_window("main")
            && !window.is_visible().unwrap_or(true)
        {
            show_quietly_or_normally(&window);
        }
    });

    // Files passed on the command line: "Open with" on Windows and Linux.
    if self_test.is_none() {
        open_files(&state, file_arguments(std::env::args().skip(1)));
    }

    #[cfg(unix)]
    {
        // `kill`, Activity Monitor's Quit and a forced logout send SIGTERM rather than a quit event. Helpers run in
        // their own process groups, so without this they would carry on after MediaForge had gone.
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) else { return };
            if terminate.recv().await.is_some() {
                begin_quit(&handle);
            }
        });
    }

    #[cfg(debug_assertions)]
    debug::install(app);

    if let Some(report) = self_test {
        self_test::start(app.clone(), report);
    }
    Ok(())
}

/// Shows the window, keeping it behind other apps' windows for background test runs and self-tests.
/// Safe to call from any thread: the work is done on the main thread, as AppKit requires.
pub fn show_quietly_or_normally(window: &tauri::WebviewWindow) {
    let quiet = background_launch() || self_test::requested().is_some();
    let target = window.clone();
    let _ = window.run_on_main_thread(move || {
        let _ = target.show();
        if !quiet {
            let _ = target.set_focus();
            return;
        }
        #[cfg(target_os = "macos")]
        if let Ok(ns_window) = target.ns_window() {
            let ns_window = ns_window as *mut objc2::runtime::AnyObject;
            let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
            unsafe {
                let _: () = objc2::msg_send![ns_window, orderBack: nil];
            }
        }
    });
}

/// Paths among command-line arguments, ignoring options.
fn file_arguments(arguments: impl Iterator<Item = String>) -> Vec<PathBuf> {
    arguments.filter(|a| !a.starts_with('-')).map(PathBuf::from).filter(|p| p.is_absolute() && p.exists()).collect()
}

fn open_files(state: &AppState, files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    state.navigate(Section::Conversions);
    if let Err(error) = commands::import_paths(state, &files, None) {
        state.alert(error.title, error.message);
    }
}

pub fn apply_appearance(app: &AppHandle, appearance: Appearance) {
    let theme = match appearance {
        Appearance::Dark => Some(tauri::Theme::Dark),
        Appearance::Light => Some(tauri::Theme::Light),
        Appearance::System => None,
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_theme(theme);
    }
}

/// Desktop notifications need no up-front permission on Windows; on macOS the system asks the first time one
/// is shown. Kept as the single place to ask, should a platform start requiring it.
pub fn prepare_notifications(state: &AppState) {
    if state.settings().notifications_enabled {
        let _ = state.handle.notification().request_permission();
    }
}

fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let app = window.app_handle();
    let Some(state) = app.try_state::<Arc<AppState>>() else { return };
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            if state.quitting.load(Ordering::SeqCst) {
                return;
            }
            if cfg!(target_os = "macos") {
                // Closing the window shouldn't abandon a queue that's still working; the app stays in the Dock.
                api.prevent_close();
                let _ = window.hide();
            } else if state.queue.pending_count(None) > 0 {
                api.prevent_close();
                confirm_quit(app);
            }
        }
        WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
            if !paths.is_empty() {
                open_files(&state, paths.clone());
            }
        }
        WindowEvent::Focused(true)
            // Picks up a tool the user installed while MediaForge was in the background.
            if !state.tools.missing().is_empty() => {
                state.tools.refresh(None);
            }
        _ => {}
    }
}

fn on_run_event(app: &AppHandle, event: RunEvent) {
    match event {
        RunEvent::ExitRequested { api, code, .. } => {
            let Some(state) = app.try_state::<Arc<AppState>>() else { return };
            if state.quitting.load(Ordering::SeqCst) || code.is_some() {
                return;
            }
            if state.queue.pending_count(None) > 0 {
                api.prevent_exit();
                confirm_quit(app);
            }
        }
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                state.quitting.store(true, Ordering::SeqCst);
                state.queue.cancel_all(None);
                state.clean_up();
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                state.show_window();
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => {
            // "Open With", drops on the Dock icon, and `open -a MediaForge file`.
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                let files: Vec<PathBuf> = urls.into_iter().filter_map(|u| u.to_file_path().ok()).collect();
                state.show_window();
                open_files(&state, files);
            }
        }
        _ => {}
    }
}

/// Asks before quitting with work unfinished.
fn confirm_quit(app: &AppHandle) {
    let Some(state) = app.try_state::<Arc<AppState>>() else { return };
    state.show_window();
    let pending = state.queue.pending_count(None);
    let headline = if pending == 1 { "A job is still in progress.".to_string() } else { format!("{pending} jobs are still in progress.") };
    let handle = app.clone();
    app.dialog()
        .message("Quitting now cancels unfinished downloads and conversions. Completed files are kept.")
        .title(headline)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Quit".into(), "Keep Working".into()))
        .show(move |quit| {
            if quit {
                begin_quit(&handle);
            }
        });
}

/// Stops every helper, then exits. Helpers run in their own process groups or job objects, so they must be
/// stopped explicitly before the app goes.
pub fn begin_quit(app: &AppHandle) {
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        app.exit(0);
        return;
    };
    if state.quitting.swap(true, Ordering::SeqCst) {
        return;
    }
    state.queue.cancel_all(None);
    let handle = app.clone();
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn(async move {
        let _ = tokio::time::timeout(Duration::from_secs(10), state.queue.wait_until_idle()).await;
        state.clean_up();
        handle.exit(0);
    });
}

/// Sends a menu action to the UI.
pub fn emit_menu_action(app: &AppHandle, action: &str) {
    let _ = app.emit(events::MENU, action);
}
