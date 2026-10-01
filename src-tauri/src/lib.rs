//! Hindsight's application. It keeps the process's memory out of crash dumps, starts the
//! recorder, and shows what the recorder is doing; the menu bar, saving and settings arrive in
//! the next pull requests.

mod recording;

use std::sync::Mutex;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // First thing: anything the process holds later (the recording, its key) must never end
    // up in a crash dump.
    hindsight_core::privacy::keep_memory_out_of_crash_dumps();

    tauri::Builder::default()
        .manage(recording::Recording(Mutex::new(None)))
        .setup(|app| {
            recording::start(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![recording::recorder_status])
        .on_window_event(|window, event| {
            // Stopping the recorder joins its threads, which drops the stream and wipes the
            // buffer and its key before the process ends.
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(recording) = window.app_handle().try_state::<recording::Recording>() {
                    recording.0.lock().expect("recording").take();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Hindsight");
}
