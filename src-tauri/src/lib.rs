//! Hindsight's application. For now it opens its window and keeps the process's memory out of
//! crash dumps; the recorder, menu bar and settings arrive in the next pull requests.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // First thing: anything the process holds later (the recording, its key) must never end
    // up in a crash dump.
    hindsight_core::privacy::keep_memory_out_of_crash_dumps();

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running Hindsight");
}
