/**
 * Hotkeys travel as the global-shortcut plugin's strings — `shift+control+alt+super+KeyA` —
 * built from the key's physical `code`, so Option changing the typed character doesn't matter.
 */

const MODIFIER_CODES = new Set([
  "ShiftLeft",
  "ShiftRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
  "CapsLock",
  "Fn",
]);

export type RecordedKey =
  | { kind: "hotkey"; hotkey: string }
  | { kind: "cancel" }
  /** A modifier on its own, or a key with no modifier: keep listening. */
  | { kind: "incomplete" };

export function fromKeyboardEvent(event: KeyboardEvent): RecordedKey {
  const modifiers = [
    event.shiftKey && "shift",
    event.ctrlKey && "control",
    event.altKey && "alt",
    event.metaKey && "super",
  ].filter(Boolean);

  if (event.code === "Escape" && modifiers.length === 0) return { kind: "cancel" };
  if (MODIFIER_CODES.has(event.code) || !event.code) return { kind: "incomplete" };
  // The backend enforces this too; checking here keeps the recorder listening instead.
  if (!event.ctrlKey && !event.altKey && !event.metaKey) return { kind: "incomplete" };

  return { kind: "hotkey", hotkey: [...modifiers, event.code].join("+") };
}

export type Platform = "macos" | "windows" | "linux";

/** From the webview's user agent: WKWebView, WebView2 and WebKitGTK each name their OS. */
export function detectPlatform(userAgent: string = navigator.userAgent): Platform {
  if (/Macintosh|Mac OS X/.test(userAgent)) return "macos";
  if (/Windows/.test(userAgent)) return "windows";
  return "linux";
}

const platform = detectPlatform();

/** Apple's symbols, in Apple's order. */
const MAC_MODIFIERS: [string, string][] = [
  ["control", "⌃"],
  ["alt", "⌥"],
  ["shift", "⇧"],
  ["super", "⌘"],
];

/** Written out, in the order Windows and Linux menus use. */
const WORDED_MODIFIERS: Record<Exclude<Platform, "macos">, [string, string][]> = {
  windows: [
    ["control", "Ctrl"],
    ["alt", "Alt"],
    ["shift", "Shift"],
    ["super", "Win"],
  ],
  linux: [
    ["control", "Ctrl"],
    ["alt", "Alt"],
    ["shift", "Shift"],
    ["super", "Super"],
  ],
};

const SHARED_KEY_NAMES: Record<string, string> = {
  Space: "Space",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backquote: "`",
};

const MAC_KEY_NAMES: Record<string, string> = {
  Escape: "⎋",
  Enter: "↩",
  Tab: "⇥",
  Backspace: "⌫",
  Delete: "⌦",
};

const WORDED_KEY_NAMES: Record<string, string> = {
  Escape: "Esc",
  Enter: "Enter",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Del",
};

function keyName(code: string, on: Platform): string {
  const names = on === "macos" ? MAC_KEY_NAMES : WORDED_KEY_NAMES;
  if (code in names) return names[code];
  if (code in SHARED_KEY_NAMES) return SHARED_KEY_NAMES[code];
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (code.startsWith("Numpad")) return `Num ${code.slice(6)}`;
  return code;
}

/** `alt+shift+KeyA` → `⌥⇧A` on macOS, `Alt+Shift+A` on Windows and Linux. */
export function formatHotkey(hotkey: string, on: Platform = platform): string {
  const parts = hotkey.split("+");
  const key = keyName(parts.pop() ?? "", on);
  if (on === "macos") {
    return (
      MAC_MODIFIERS.filter(([name]) => parts.includes(name))
        .map(([, symbol]) => symbol)
        .join("") + key
    );
  }
  return [
    ...WORDED_MODIFIERS[on].filter(([name]) => parts.includes(name)).map(([, word]) => word),
    key,
  ].join("+");
}

/** The modifiers a hotkey must include (any one), as the user sees them. */
export function requiredModifiers(on: Platform = platform): string {
  return on === "macos" ? "⌘, ⌥ or ⌃" : "Ctrl, Alt or " + (on === "windows" ? "Win" : "Super");
}

/**
 * Who already uses a shortcut: every app, by convention (Hindsight takes it from all of them), or
 * the operating system (which usually keeps it, so the hotkey won't register at all).
 */
type Owner = { by: "apps" | "system"; does: string };

const apps = (does: string): Owner => ({ by: "apps", does });
const system = (does: string): Owner => ({ by: "system", does });

/** On Windows and Linux, Ctrl does what ⌘ does on a Mac. */
const CONTROL_KEY_CONVENTIONS: Record<string, Owner> = {
  "control+KeyA": apps("Select All"),
  "control+KeyC": apps("Copy"),
  "control+KeyF": apps("Find"),
  "control+KeyJ": apps("Downloads in most browsers"),
  "control+KeyL": apps("the address bar in most browsers"),
  "control+KeyN": apps("New"),
  "control+KeyO": apps("Open"),
  "control+KeyP": apps("Print"),
  "control+KeyR": apps("Reload in most browsers"),
  "control+KeyS": apps("Save"),
  "control+KeyT": apps("New Tab"),
  "control+KeyV": apps("Paste"),
  "control+KeyW": apps("Close"),
  "control+KeyX": apps("Cut"),
  "control+KeyY": apps("Redo"),
  "control+KeyZ": apps("Undo"),
  "shift+control+KeyZ": apps("Redo"),
  "shift+control+KeyS": apps("Save As"),
  "control+Tab": apps("switching tabs"),
  "alt+F4": system("closing the window"),
  "alt+Tab": system("the app switcher"),
};

const COMMON_SHORTCUTS: Record<Platform, Record<string, Owner>> = {
  macos: {
    "super+KeyA": apps("Select All"),
    "super+KeyC": apps("Copy"),
    "super+KeyF": apps("Find"),
    "super+KeyH": apps("Hide"),
    "super+KeyJ": apps("Downloads in most browsers"),
    "super+KeyL": apps("the address bar in most browsers"),
    "super+KeyM": apps("Minimize"),
    "super+KeyN": apps("New"),
    "super+KeyO": apps("Open"),
    "super+KeyP": apps("Print"),
    "super+KeyQ": apps("Quit"),
    "super+KeyR": apps("Reload in most browsers"),
    "super+KeyS": apps("Save"),
    "super+KeyT": apps("New Tab"),
    "super+KeyV": apps("Paste"),
    "super+KeyW": apps("Close"),
    "super+KeyX": apps("Cut"),
    "super+KeyZ": apps("Undo"),
    "shift+super+KeyZ": apps("Redo"),
    "shift+super+KeyS": apps("Save As"),
    "super+Comma": apps("Settings"),
    "super+Space": system("Spotlight"),
    "super+Tab": system("the app switcher"),
    "shift+super+Digit3": system("a screenshot"),
    "shift+super+Digit4": system("a screenshot"),
    "shift+super+Digit5": system("the screenshot toolbar"),
    "control+super+KeyQ": system("locking the screen"),
  },
  windows: {
    ...CONTROL_KEY_CONVENTIONS,
    "super+KeyD": system("showing the desktop"),
    "super+KeyE": system("File Explorer"),
    "super+KeyI": system("Settings"),
    "super+KeyL": system("locking the screen"),
    "super+KeyR": system("the Run dialog"),
    "super+KeyV": system("clipboard history"),
    "super+Tab": system("Task View"),
    "super+Space": system("switching keyboard layouts"),
    "shift+super+KeyS": system("a screenshot"),
    "shift+control+Escape": system("Task Manager"),
  },
  linux: {
    ...CONTROL_KEY_CONVENTIONS,
    "alt+F2": system("the run command dialog"),
    "control+alt+KeyT": system("opening a terminal"),
    "control+alt+Delete": system("logging out"),
    "super+KeyL": system("locking the screen"),
    "super+Space": system("switching input sources"),
    "super+Tab": system("the app switcher"),
  },
};

const SYSTEM_NAMES: Record<Platform, string> = {
  macos: "macOS",
  windows: "Windows",
  linux: "your desktop",
};

/** A warning when `hotkey` is one other apps or the system already rely on. */
export function commonConflict(hotkey: string | null, on: Platform = platform): string | null {
  const owner = hotkey ? COMMON_SHORTCUTS[on][hotkey] : undefined;
  if (!hotkey || !owner) return null;
  const shown = formatHotkey(hotkey, on);
  return owner.by === "apps"
    ? `${shown} is ${owner.does} in other apps — Hindsight takes it from all of them while it runs.`
    : `${shown} is ${owner.does} in ${SYSTEM_NAMES[on]}, which may keep it — the hotkey might not work.`;
}
