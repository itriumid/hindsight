<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import Settings, { type SettingsView } from "$lib/components/Settings.svelte";
  import Welcome from "$lib/components/Welcome.svelte";

  interface Status {
    microphone: string | null;
    role: "primary" | "fallback" | "systemDefault" | null;
    bufferedSeconds: number;
    bufferSeconds: number;
  }

  type Notice =
    | { kind: "recording"; microphone: string; role: string }
    | { kind: "lost"; microphone: string; role: string; reason: string }
    | { kind: "waiting"; retryInSeconds: number };

  interface Saved {
    path: string;
    fileName: string;
    seconds: number;
  }

  let settings = $state<SettingsView | null>(null);
  let view = $state<"home" | "settings">("home");
  let status = $state<Status | null>(null);
  let folder = $state<string | null>(null);
  let waiting = $state<number | null>(null);
  let history = $state<{ at: Date; text: string }[]>([]);

  function describe(notice: Notice): string {
    switch (notice.kind) {
      case "recording":
        return `Recording from ${notice.microphone}${notice.role === "fallback" ? " (the fallback)" : ""}`;
      case "lost":
        return `Lost ${notice.microphone}: ${notice.reason}`;
      case "waiting":
        return notice.retryInSeconds > 0
          ? `No microphone is working; trying again in ${notice.retryInSeconds} s`
          : "No microphone is working; trying again";
    }
  }

  function duration(seconds: number): string {
    const whole = Math.floor(seconds);
    const hours = Math.floor(whole / 3600);
    const minutes = Math.floor((whole % 3600) / 60);
    const rest = whole % 60;
    if (hours > 0) return `${hours} h ${minutes} min`;
    if (minutes > 0) return `${minutes} min ${rest} s`;
    return `${rest} s`;
  }

  async function chooseFolder() {
    folder = await invoke<string | null>("choose_clips_folder");
  }

  function save(minutes: number) {
    invoke("save_last", { minutes });
  }

  async function refresh() {
    status = await invoke<Status>("recorder_status");
    if (status.microphone) waiting = null;
  }

  async function loadSettings() {
    settings = await invoke<SettingsView>("get_settings");
    folder = settings.clipsFolder;
  }

  onMount(() => {
    loadSettings();
    refresh();
    invoke<string | null>("clips_folder").then((value) => (folder = value));
    const timer = setInterval(refresh, 1000);
    const stopSaved = listen<Saved>("saved", ({ payload }) => {
      history = [{ at: new Date(), text: `Saved ${payload.fileName}` }, ...history].slice(0, 6);
      invoke<string | null>("clips_folder").then((value) => (folder = value));
    });
    const stop = listen<Notice>("recorder", ({ payload }) => {
      waiting = payload.kind === "waiting" ? payload.retryInSeconds : null;
      history = [{ at: new Date(), text: describe(payload) }, ...history].slice(0, 6);
      refresh();
    });
    return () => {
      clearInterval(timer);
      stop.then((unlisten) => unlisten());
      stopSaved.then((unlisten) => unlisten());
    };
  });
</script>

{#if settings && !settings.welcomed}
  <Welcome onstart={loadSettings} />
{:else if settings}
<main>
  <header>
    <h1>{view === "home" ? "Hindsight" : "Settings"}</h1>
    <button class="link" onclick={() => (view = view === "home" ? "settings" : "home")}>
      {view === "home" ? "Settings" : "Done"}
    </button>
  </header>

  {#if view === "settings"}
    <Settings bind:settings />
  {:else}

  {#if status?.microphone}
    <p class="state recording">
      <span class="dot" aria-hidden="true"></span>
      Recording from {status.microphone}{status.role === "fallback" ? " (the fallback)" : ""}
    </p>
  {:else}
    <p class="state">Not recording right now</p>
    {#if waiting !== null}
      <p class="hint">
        No microphone is sending sound. If this keeps happening, check that Hindsight may use the
        microphone: System Settings, Privacy &amp; Security, Microphone.
      </p>
    {/if}
  {/if}

  {#if status}
    <div class="buffer">
      <p>
        Holding the last <strong>{duration(status.bufferedSeconds)}</strong>
        of up to {duration(status.bufferSeconds)}, in memory only.
      </p>
      <div class="bar" aria-hidden="true">
        <div class="fill" style:width="{Math.min(100, (status.bufferedSeconds / status.bufferSeconds) * 100)}%"></div>
      </div>
    </div>
  {/if}

  {#if history.length > 0}
    <section>
      <h2>Recently</h2>
      <ol>
        {#each history as entry (entry.at.getTime() + entry.text)}
          <li>
            <time>{entry.at.toLocaleTimeString()}</time>
            {entry.text}
          </li>
        {/each}
      </ol>
    </section>
  {/if}

  <section class="save">
    <h2>Save</h2>
    <div class="buttons">
      <button onclick={() => save(1)}>Last minute</button>
      <button onclick={() => save(5)}>Last 5 minutes</button>
      <button class="primary" onclick={() => save(15)}>Last 15 minutes</button>
    </div>
    <p class="hint">Or from the menu bar icon, without opening this window.</p>
  </section>

  <section class="folder">
    <h2>Clips folder</h2>
    {#if folder}
      <p class="path" title={folder}>{folder}</p>
      <div class="buttons">
        <button onclick={chooseFolder}>Change…</button>
        <button onclick={() => invoke("reveal_clips_folder")}>Show</button>
      </div>
    {:else}
      <p class="hint">Not chosen yet. Hindsight asks the first time you save.</p>
      <div class="buttons">
        <button onclick={chooseFolder}>Choose now…</button>
      </div>
    {/if}
  </section>
  {/if}
</main>
{/if}

<style>
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  button.link {
    padding: var(--space-1) var(--space-2);
    background: transparent;
    color: var(--text);
    border: 1px solid var(--border);
  }

  main {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: 560px;
    margin: 0 auto;
    padding: var(--space-6);
  }

  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }

  h2 {
    margin: 0 0 var(--space-2);
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  p {
    margin: 0;
  }

  .state {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: 15px;
    font-weight: 500;
  }

  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--accent);
    outline: 1px solid var(--accent-edge);
  }

  .hint {
    color: var(--muted);
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  button {
    padding: var(--space-1) var(--space-3);
    background: var(--elevated);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font: inherit;
  }

  button:hover {
    border-color: var(--accent-edge);
  }

  button.primary {
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
  }

  .save,
  .folder {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .path {
    overflow: hidden;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }

  .buffer {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--elevated);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    background: var(--accent);
    transition: width var(--duration) var(--ease);
  }

  ol {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    color: var(--text);
  }

  time {
    margin-right: var(--space-2);
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
</style>
