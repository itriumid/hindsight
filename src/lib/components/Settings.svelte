<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { PalettePicker, ThemeSwitcher } from "@itrium/palettes";
  import HotkeyRecorder from "$lib/components/HotkeyRecorder.svelte";
  import { theme } from "$lib/theme";

  export interface SettingsView {
    welcomed: boolean;
    clipsFolder: string | null;
    microphone: string | null;
    fallbackMicrophone: string | null;
    bufferMinutes: number;
    saveHotkey: string | null;
    saveHotkeyMinutes: number;
    showInDock: boolean;
    showInMenuBar: boolean;
    launchAtLogin: boolean;
    hasDock: boolean;
  }

  let { settings = $bindable() }: { settings: SettingsView } = $props();

  const BUFFER_CHOICES = [15, 30, 60, 120, 180];

  // Every choice keeps a way back to the window: on macOS "neither" isn't offered.
  const presences = $derived(
    settings.hasDock
      ? [
          { id: "menu-bar", label: "Menu bar only", menuBar: true, dock: false },
          { id: "dock", label: "Dock only", menuBar: false, dock: true },
          { id: "both", label: "Menu bar and Dock", menuBar: true, dock: true },
        ]
      : [
          { id: "tray", label: "In the system tray", menuBar: true, dock: false },
          { id: "hidden", label: "Hidden", menuBar: false, dock: false },
        ],
  );
  const SAVE_CHOICES = [1, 5, 15];

  let microphones = $state<string[]>([]);
  let error = $state("");
  /** A new buffer length waiting for confirmation, since changing it wipes the buffer. */
  let pendingBuffer = $state<number | null>(null);

  function label(minutes: number): string {
    if (minutes % 60 === 0) {
      const hours = minutes / 60;
      return `${hours} hour${hours === 1 ? "" : "s"}`;
    }
    return `${minutes} minutes`;
  }

  /** Runs a setting change; shows why it was refused, and keeps the last good settings. */
  async function change(command: string, args: Record<string, unknown>) {
    error = "";
    try {
      settings = await invoke<SettingsView>(command, args);
    } catch (caught) {
      error = String(caught);
      settings = await invoke<SettingsView>("get_settings");
    }
  }

  function chooseMicrophones(microphone: string | null, fallback: string | null) {
    change("set_microphones", { microphone, fallback });
  }

  function chooseBuffer(minutes: number) {
    pendingBuffer = minutes === settings.bufferMinutes ? null : minutes;
  }

  function confirmBuffer() {
    if (pendingBuffer !== null) change("set_buffer_minutes", { minutes: pendingBuffer });
    pendingBuffer = null;
  }

  async function saveHotkey(hotkey: string | null) {
    settings = await invoke<SettingsView>("set_save_hotkey", {
      hotkey,
      minutes: settings.saveHotkeyMinutes,
    });
  }

  onMount(async () => {
    microphones = await invoke<string[]>("list_microphones");
  });
</script>

<div class="settings">
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <section>
    <h2>Microphone</h2>
    <label>
      Record from
      <select
        value={settings.microphone ?? ""}
        onchange={(event) => chooseMicrophones(event.currentTarget.value || null, settings.fallbackMicrophone)}
      >
        <option value="">System default</option>
        {#each microphones as name (name)}<option value={name}>{name}</option>{/each}
        {#if settings.microphone && !microphones.includes(settings.microphone)}
          <option value={settings.microphone}>{settings.microphone} (not connected)</option>
        {/if}
      </select>
    </label>
    <label>
      If it disconnects, use
      <select
        value={settings.fallbackMicrophone ?? ""}
        onchange={(event) => chooseMicrophones(settings.microphone, event.currentTarget.value || null)}
      >
        <option value="">System default</option>
        {#each microphones as name (name)}<option value={name}>{name}</option>{/each}
        {#if settings.fallbackMicrophone && !microphones.includes(settings.fallbackMicrophone)}
          <option value={settings.fallbackMicrophone}>{settings.fallbackMicrophone} (not connected)</option>
        {/if}
      </select>
    </label>
    <p class="hint">Hindsight goes back to your choice by itself when it reconnects.</p>
  </section>

  <section>
    <h2>How far back</h2>
    <label>
      Keep the last
      <select
        value={pendingBuffer ?? settings.bufferMinutes}
        onchange={(event) => chooseBuffer(Number(event.currentTarget.value))}
      >
        {#each BUFFER_CHOICES as minutes (minutes)}<option value={minutes}>{label(minutes)}</option>{/each}
      </select>
    </label>
    {#if pendingBuffer !== null}
      <div class="confirm" role="alert">
        <span>This wipes what Hindsight holds now and starts again.</span>
        <button onclick={confirmBuffer}>Change it</button>
        <button onclick={() => (pendingBuffer = null)}>Keep {label(settings.bufferMinutes)}</button>
      </div>
    {:else}
      <p class="hint">Three hours takes about 22 MB of memory.</p>
    {/if}
  </section>

  <section>
    <h2>Save shortcut</h2>
    <div class="row">
      <HotkeyRecorder hotkey={settings.saveHotkey} onsave={saveHotkey} />
      <label>
        saves the last
        <select
          value={settings.saveHotkeyMinutes}
          onchange={(event) =>
            change("set_save_hotkey", { hotkey: settings.saveHotkey, minutes: Number(event.currentTarget.value) })}
        >
          {#each SAVE_CHOICES as minutes (minutes)}
            <option value={minutes}>{minutes === 1 ? "minute" : `${minutes} minutes`}</option>
          {/each}
        </select>
      </label>
    </div>
    <p class="hint">Works while other apps are in front. There's none until you set one.</p>
  </section>

  <section>
    <h2>Where Hindsight shows up</h2>
    <div class="choices" role="radiogroup" aria-label="Where Hindsight shows up">
      {#each presences as presence (presence.id)}
        <label class="check">
          <input
            type="radio"
            name="presence"
            checked={settings.showInMenuBar === presence.menuBar && (!settings.hasDock || settings.showInDock === presence.dock)}
            onchange={() => change("set_presence", { menuBar: presence.menuBar, dock: presence.dock })}
          />
          {presence.label}
        </label>
      {/each}
    </div>
    <p class="hint">
      {settings.hasDock
        ? "Your Mac's microphone indicator shows Hindsight is recording either way."
        : "With the icon hidden, open Hindsight again to get this window back."}
    </p>
    <label class="check">
      <input
        type="checkbox"
        checked={settings.launchAtLogin}
        onchange={(event) => change("set_launch_at_login", { on: event.currentTarget.checked })}
      />
      Start Hindsight when I log in
    </label>
  </section>

  <section>
    <h2>Colors</h2>
    <div class="row">
      <PalettePicker {theme} label="Palette" />
      <ThemeSwitcher {theme} />
    </div>
  </section>

  <section>
    <h2>Your data</h2>
    <p>
      Hindsight keeps its settings in its own folder. Removing them also turns off launch at login
      and the shortcut, then quits. Clips you saved stay where they are.
    </p>
    <div class="row">
      <button class="danger" onclick={() => invoke("remove_all_data")}>Remove all Hindsight data…</button>
    </div>
  </section>
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  section {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  h2 {
    margin: 0;
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  p {
    margin: 0;
  }

  label {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-3);
  }

  select {
    max-width: 280px;
    padding: var(--space-1) var(--space-2);
    background: var(--elevated);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font: inherit;
  }

  input[type="checkbox"],
  input[type="radio"] {
    accent-color: var(--accent-edge);
  }

  .choices {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .hint {
    color: var(--muted);
  }

  .confirm {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2);
    background: var(--elevated);
    border-left: 2px solid var(--accent-edge);
  }

  .error {
    padding: var(--space-2);
    background: var(--elevated);
    border-left: 2px solid var(--accent-edge);
  }

  button {
    padding: var(--space-1) var(--space-3);
    background: var(--elevated);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font: inherit;
  }

  button.danger:hover {
    border-color: var(--accent-edge);
  }
</style>
