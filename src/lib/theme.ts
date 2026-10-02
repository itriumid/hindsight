import { getCurrentWindow } from "@tauri-apps/api/window";
import { Theme } from "@itrium/palettes";

/** The chosen theme and palette. app.html reads the same keys before the first paint. */
export const theme = new Theme({
  storagePrefix: "hindsight",
  // Keeps the native title bar in step; null follows the system.
  onApply: (choice) => {
    getCurrentWindow()
      .setTheme(choice)
      .catch(() => {});
  },
});
