//! The menu bar (macOS) or tray (Windows, Linux) icon: shows that Hindsight is recording and
//! holds the save options, so most of the time the window never needs opening.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::saving;

pub struct TrayItems {
    status: MenuItem<Wry>,
    buffered: MenuItem<Wry>,
    show_last: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let buffered = MenuItem::with_id(app, "buffered", "Holding nothing yet", false, None::<&str>)?;
    let save_one = MenuItem::with_id(app, "save-1", "Save the last minute", true, None::<&str>)?;
    let save_five = MenuItem::with_id(app, "save-5", "Save the last 5 minutes", true, None::<&str>)?;
    let save_fifteen = MenuItem::with_id(app, "save-15", "Save the last 15 minutes", true, None::<&str>)?;
    let show_last = MenuItem::with_id(app, "show-last", "Show the last clip in its folder", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Hindsight", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Hindsight", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &buffered,
            &PredefinedMenuItem::separator(app)?,
            &save_one,
            &save_five,
            &save_fifteen,
            &PredefinedMenuItem::separator(app)?,
            &show_last,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // macOS tints a template image to suit the menu bar; elsewhere the full-color icon reads on
    // light and dark trays alike.
    #[cfg(target_os = "macos")]
    let icon = Image::from_bytes(include_bytes!("../icons/tray-template@2x.png"))?;
    #[cfg(not(target_os = "macos"))]
    let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;

    TrayIconBuilder::with_id("hindsight")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Hindsight")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "save-1" => saving::save_last(app, 1),
            "save-5" => saving::save_last(app, 5),
            "save-15" => saving::save_last(app, 15),
            "show-last" => saving::reveal_last(app),
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    app.manage(TrayItems { status, buffered, show_last });
    Ok(())
}

/// The menu's second line: how much the buffer holds, like the window shows.
pub fn set_buffered(app: &AppHandle, text: String) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(items) = handle.try_state::<TrayItems>() {
            let _ = items.buffered.set_text(text);
        }
    });
}

/// The first line of the menu, which says what Hindsight is doing.
pub fn set_status(app: &AppHandle, text: String) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(items) = handle.try_state::<TrayItems>() {
            let _ = items.status.set_text(text);
        }
    });
}

pub fn enable_show_last(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(items) = handle.try_state::<TrayItems>() {
            let _ = items.show_last.set_enabled(true);
        }
    });
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
