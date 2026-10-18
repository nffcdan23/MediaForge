// Every command the window may call is declared here. Tauri turns each into a permission
// ("allow-<name>"), and capabilities/main.json grants exactly these to the main window.
const COMMANDS: &[&str] = &[
    "bootstrap",
    "update_settings",
    "choose_destination",
    "open_destination",
    "choose_tool_path",
    "reset_tool_path",
    "refresh_tools",
    "acknowledge_terms",
    "set_section",
    "quit_app",
    "inspect_links",
    "resolve_link",
    "cancel_resolve",
    "enqueue_downloads",
    "import_files",
    "choose_files_to_convert",
    "cancel_job",
    "retry_job",
    "remove_job",
    "cancel_all",
    "clear_finished",
    "job_log",
    "reveal_path",
    "open_path",
    "copy_text",
    "read_clipboard",
    "open_third_party_notices",
    "remove_history",
    "clear_history",
    "repeat_history",
    "thumbnail",
    "waveform",
    "preview_source",
    "licensing_summary",
    "show_main_window",
];

fn main() {
    // The build number ("1.1.0 (4)") lives with the version in tauri.conf.json; the app reads it at compile time.
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string("tauri.conf.json").expect("tauri.conf.json")).expect("valid JSON");
    let build = config["bundle"]["macOS"]["bundleVersion"].as_str().unwrap_or("").to_string();
    println!("cargo:rustc-env=MEDIAFORGE_BUILD_NUMBER={build}");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)))
        .expect("failed to run the Tauri build script");
}
