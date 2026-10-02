//! "Start Hindsight when I log in": the login entry, kept pointing at the copy of Hindsight that
//! actually runs. The entry stores an absolute path, so a moved app, an AppImage updated under a
//! new name, or a test build that turned it on would otherwise leave it starting a copy that's
//! gone, or the wrong one.

use std::path::PathBuf;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::ManagerExt;

use crate::settings::SettingsStore;

/// Launch at login starts Hindsight with this, so it starts quietly in the menu bar.
pub const BACKGROUND: &str = "--background";

/// The launch agent's name and label on macOS. Agents there are named after their app's
/// identifier, so this one can't collide with another app's. Elsewhere the name shows up in
/// startup lists, so it stays "Hindsight".
#[cfg(target_os = "macos")]
const IDENTIFIER: &str = "id.itrium.hindsight";

/// What Hindsight 0.1.0 named its launch agent on macOS.
#[cfg(target_os = "macos")]
const OLD_NAME: &str = "Hindsight";

pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let builder = tauri_plugin_autostart::Builder::new().arg(BACKGROUND);
    // A launch agent, not AppleScript, which would ask to control System Events.
    #[cfg(target_os = "macos")]
    let builder = builder
        .macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent)
        .app_name(IDENTIFIER);
    builder.build()
}

/// Turns launch at login on or off, and remembers which copy it starts.
pub fn set(app: &AppHandle, on: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if on { manager.enable() } else { manager.disable() }.map_err(|error| format!("couldn't change launch at login: {error}"))?;
    let target = if on { running_copy(app) } else { None };
    app.state::<SettingsStore>().update(|settings| {
        settings.launch_at_login = on;
        settings.launch_at_login_target = target;
    })?;
    Ok(())
}

/// Turns launch at login off under every name Hindsight has used.
pub fn turn_off(app: &AppHandle) {
    let _ = app.autolaunch().disable();
    if let Some(old) = old_launch_agent(app) {
        let _ = std::fs::remove_file(old);
    }
}

/// At start-up: if launch at login is on but starts some other copy, point it at this one, and
/// move a 0.1.0 launch agent to its new name. Debug builds leave it alone, so running one never
/// takes it over.
pub fn keep_current(app: &AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let Some(running) = running_copy(app) else {
        return;
    };
    let old = old_launch_agent(app);
    if old.is_none() {
        let on = app.autolaunch().is_enabled().unwrap_or(false);
        if !on || app.state::<SettingsStore>().get().launch_at_login_target.as_ref() == Some(&running) {
            return;
        }
    }
    // The new entry first, so a failure leaves the old one working.
    if set(app, true).is_ok()
        && let Some(old) = old
    {
        let _ = std::fs::remove_file(old);
    }
}

/// What the login entry would start if it were turned on now: this executable, or on Linux the
/// AppImage it runs from, the same paths the plugin registers. `None` for a copy macOS is
/// running from a temporary, randomized place (App Translocation, for a quarantined app opened
/// where it was downloaded): that path is gone once Hindsight quits.
fn running_copy(app: &AppHandle) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(appimage) = app.env().appimage {
        return Some(appimage.into());
    }
    let _ = app;
    let executable = std::env::current_exe().ok()?;
    if cfg!(target_os = "macos") && executable.to_string_lossy().contains("/AppTranslocation/") {
        return None;
    }
    Some(executable)
}

/// 0.1.0's launch agent, if there is one and it's Hindsight's, not another app's that's also
/// called Hindsight.
#[cfg(target_os = "macos")]
fn old_launch_agent(app: &AppHandle) -> Option<PathBuf> {
    let file = app.path().home_dir().ok()?.join("Library/LaunchAgents").join(format!("{OLD_NAME}.plist"));
    let contents = std::fs::read_to_string(&file).ok()?;
    starts_hindsight(&contents).then_some(file)
}

#[cfg(not(target_os = "macos"))]
fn old_launch_agent(_app: &AppHandle) -> Option<PathBuf> {
    None
}

/// Does this launch agent start a `hindsight` executable quietly, the way Hindsight's does?
#[cfg(target_os = "macos")]
fn starts_hindsight(plist: &str) -> bool {
    plist.contains("/hindsight</string>") && plist.contains(&format!("<string>{BACKGROUND}</string>"))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_launch_agent_is_named_after_the_identifier() {
        let configuration: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(configuration["identifier"], IDENTIFIER);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_hindsights_own_old_launch_agent_is_moved() {
        // As 0.1.0 wrote it, and as macOS rewrites it.
        let written = r#"<plist version="1.0">
  <dict>
  <key>Label</key>
  <string>Hindsight</string>
  <key>AssociatedBundleIdentifiers</key>
  <array></array>
  <key>ProgramArguments</key>
  <array><string>/Applications/Hindsight.app/Contents/MacOS/hindsight</string><string>--background</string></array>
  <key>RunAtLoad</key>
  <true/>
  </dict>
</plist>"#;
        let rewritten = "<dict>\n\t<key>Label</key>\n\t<string>Hindsight</string>\n\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/Users/someone/Applications/Hindsight.app/Contents/MacOS/hindsight</string>\n\t\t<string>--background</string>\n\t</array>\n</dict>";
        assert!(starts_hindsight(written));
        assert!(starts_hindsight(rewritten));

        // Another app called Hindsight keeps its agent.
        let other = "<key>Label</key><string>Hindsight</string><key>ProgramArguments</key><array><string>/Applications/Hindsight.app/Contents/MacOS/Hindsight</string><string>--minimized</string></array>";
        assert!(!starts_hindsight(other));
    }
}
