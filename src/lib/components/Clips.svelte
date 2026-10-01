<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  interface Clip {
    path: string;
    name: string;
    seconds: number;
    bytes: number;
    savedAt: number;
  }

  interface Playback {
    path: string | null;
    positionSeconds: number;
    lengthSeconds: number;
    playing: boolean;
  }

  let { folder }: { folder: string | null } = $props();

  let clips = $state<Clip[]>([]);
  let playback = $state<Playback>({ path: null, positionSeconds: 0, lengthSeconds: 0, playing: false });
  let error = $state("");
  /** While dragging the seek bar, the bar shows the drag, not the player. */
  let dragging = $state<number | null>(null);

  function time(seconds: number): string {
    const whole = Math.floor(seconds);
    const hours = Math.floor(whole / 3600);
    const minutes = Math.floor((whole % 3600) / 60);
    const rest = String(whole % 60).padStart(2, "0");
    return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}` : `${minutes}:${rest}`;
  }

  function size(bytes: number): string {
    return bytes >= 1_000_000 ? `${(bytes / 1_000_000).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1000))} KB`;
  }

  function when(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  }

  async function load() {
    clips = await invoke<Clip[]>("list_clips");
  }

  async function poll() {
    playback = await invoke<Playback>("playback_state");
  }

  async function run(command: string, args: Record<string, unknown> = {}) {
    error = "";
    try {
      await invoke(command, args);
    } catch (caught) {
      error = String(caught);
    }
    poll();
  }

  function playOrPause(clip: Clip) {
    if (playback.path === clip.path) run("toggle_playback");
    else run("play_clip", { path: clip.path });
  }

  async function remove(clip: Clip) {
    error = "";
    try {
      if (await invoke<boolean>("delete_clip", { path: clip.path })) await load();
    } catch (caught) {
      error = String(caught);
    }
    poll();
  }

  function seek(seconds: number) {
    dragging = null;
    run("seek_playback", { seconds });
  }

  $effect(() => {
    // Reload whenever the folder changes.
    void folder;
    load();
  });

  onMount(() => {
    const timer = setInterval(() => {
      if (playback.path) poll();
    }, 250);
    const stop = listen("saved", () => load());
    return () => {
      clearInterval(timer);
      stop.then((unlisten) => unlisten());
      invoke("stop_playback");
    };
  });
</script>

<section class="clips">
  <h2>Clips</h2>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if !folder}
    <p class="hint">Saved clips show up here once you've chosen a clips folder.</p>
  {:else if clips.length === 0}
    <p class="hint">No clips yet. Save one from the menu bar or the buttons above.</p>
  {:else}
    <ol>
      {#each clips as clip (clip.path)}
        {@const current = playback.path === clip.path}
        <li class:current>
          <div class="row">
            <button
              class="play"
              aria-label={current && playback.playing ? `Pause ${clip.name}` : `Play ${clip.name}`}
              onclick={() => playOrPause(clip)}>{current && playback.playing ? "❙❙" : "▶"}</button
            >
            <div class="details">
              <span class="name" title={clip.name}>{when(clip.savedAt)}</span>
              <span class="meta">{time(clip.seconds)} · {size(clip.bytes)}</span>
            </div>
            <div class="actions">
              <button onclick={() => run("reveal_clip", { path: clip.path })}>Show</button>
              <button onclick={() => run("export_clip", { path: clip.path })}>Export as WAV…</button>
              <button onclick={() => remove(clip)}>Delete…</button>
            </div>
          </div>
          {#if current}
            <div class="seek">
              <span>{time(dragging ?? playback.positionSeconds)}</span>
              <input
                type="range"
                min="0"
                max={playback.lengthSeconds}
                step="0.1"
                value={dragging ?? playback.positionSeconds}
                aria-label="Position"
                oninput={(event) => (dragging = Number(event.currentTarget.value))}
                onchange={(event) => seek(Number(event.currentTarget.value))}
              />
              <span>{time(playback.lengthSeconds)}</span>
            </div>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  .clips {
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

  .hint {
    color: var(--muted);
  }

  .error {
    padding: var(--space-2);
    background: var(--elevated);
    border-left: 2px solid var(--accent-edge);
  }

  ol {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }

  li.current {
    border-color: var(--accent-edge);
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-3);
  }

  .details {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 160px;
  }

  .name {
    overflow: hidden;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    color: var(--muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }

  button {
    padding: var(--space-1) var(--space-2);
    background: var(--elevated);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font: inherit;
    font-size: 12px;
  }

  button:hover {
    border-color: var(--accent-edge);
  }

  button.play {
    width: 32px;
    height: 32px;
    padding: 0;
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
    border-radius: 50%;
    font-size: 12px;
  }

  .seek {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
  }
</style>
