//! The macOS menu bar, with the Swift app's commands and shortcuts. Windows has no menu bar: the same
//! shortcuts are handled by the window itself (see `src/state/shortcuts.ts`).

use std::sync::Arc;

use tauri::menu::{AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Manager};

use super::state::AppState;
use super::{commands, emit_menu_action};
use crate::models::Section;

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let item = |id: &str, title: &str, accelerator: Option<&str>| {
        let mut builder = MenuItemBuilder::with_id(id, title);
        if let Some(accelerator) = accelerator {
            builder = builder.accelerator(accelerator);
        }
        builder.build(app)
    };

    let about = AboutMetadataBuilder::new()
        .name(Some("MediaForge"))
        .version(Some(app.package_info().version.to_string()))
        .short_version(Some(commands::build_number(app)))
        .copyright(Some("Copyright © 2026 C2 Software. All rights reserved."))
        .build();

    let app_menu = SubmenuBuilder::new(app, "MediaForge")
        .about(Some(about))
        .separator()
        .item(&item("settings", "Settings…", Some("Cmd+,"))?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("import", "Import Files…", Some("Cmd+O"))?)
        .item(&item("import-folder", "Import Folder…", Some("Cmd+Shift+O"))?)
        .item(&item("paste", "Paste Link", Some("Cmd+Shift+V"))?)
        .separator()
        .item(&item("destination", "Open Destination Folder", Some("Cmd+Shift+D"))?)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit").undo().redo().separator().cut().copy().paste().select_all().build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item("section-downloads", "Downloads", Some("Cmd+1"))?)
        .item(&item("section-conversions", "Conversions", Some("Cmd+2"))?)
        .item(&item("section-history", "History", Some("Cmd+3"))?)
        .item(&item("section-settings", "Settings", Some("Cmd+4"))?)
        .separator()
        .fullscreen()
        .build()?;
    let queue = SubmenuBuilder::new(app, "Queue")
        .item(&item("cancel-all", "Cancel All", None)?)
        .item(&item("clear-finished", "Clear Finished", None)?)
        .separator()
        .item(&item("toggle-preview", "Play/Pause Preview", Some("Cmd+Shift+P"))?)
        .build()?;
    let window = SubmenuBuilder::new(app, "Window").minimize().maximize().separator().close_window().build()?;
    let help = SubmenuBuilder::new(app, "Help")
        .item(&item("terms", "Responsible Use…", None)?)
        .item(&item("check-tools", "Check Tools", None)?)
        .build()?;

    let menu = MenuBuilder::new(app).items(&[&app_menu, &file, &edit, &view, &queue, &window, &help]).build()?;
    app.set_menu(menu)?;

    app.on_menu_event(|app, event| {
        let Some(state) = app.try_state::<Arc<AppState>>() else { return };
        match event.id().as_ref() {
            "settings" | "section-settings" => state.navigate(Section::Settings),
            "section-downloads" => state.navigate(Section::Downloads),
            "section-conversions" => state.navigate(Section::Conversions),
            "section-history" => state.navigate(Section::History),
            "destination" => {
                let destination = state.destination();
                let _ = std::fs::create_dir_all(&destination);
                let _ = tauri_plugin_opener::OpenerExt::opener(app).open_path(destination.display().to_string(), None::<&str>);
            }
            "cancel-all" => state.queue.cancel_all(None),
            "clear-finished" => state.queue.clear_finished(None),
            "check-tools" => {
                state.tools.refresh(None);
                state.navigate(Section::Settings);
            }
            other @ ("import" | "import-folder" | "paste" | "toggle-preview" | "terms") => {
                state.show_window();
                emit_menu_action(app, other);
            }
            _ => {}
        }
    });
    Ok(())
}
