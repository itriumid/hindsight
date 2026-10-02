<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  interface View {
    generation: number;
    seconds: number;
    endsAt: number;
    bars: number;
  }

  interface Levels {
    generation: number;
    bars: [number, number][];
  }

  interface Playback {
    path: string | null;
    positionSeconds: number;
    playing: boolean;
  }

  let { ondone }: { ondone: () => void } = $props();

  /** Quieter than this draws as the shortest bar; 0 dB is the tallest. */
  const FLOOR_DB = -60;
  /** The selection never gets shorter than this. */
  const SHORTEST = 1;

  let view = $state<View | null>(null);
  let error = $state("");
  /** Decibels per bar; null until worked out. */
  let levels = $state<(number | null)[]>([]);
  let start = $state(0);
  let end = $state(0);
  let playhead = $state<number | null>(null);
  let playing = $state(false);
  let track = $state<HTMLDivElement>();
  let dragging: "start" | "end" | null = null;

  const length = $derived(Math.max(0, end - start));

  function clock(seconds: number): string {
    if (!view) return "";
    const at = new Date(view.endsAt - (view.seconds - seconds) * 1000);
    return at.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  }

  function amount(seconds: number): string {
    const whole = Math.round(seconds);
    const hours = Math.floor(whole / 3600);
    const minutes = Math.floor((whole % 3600) / 60);
    const rest = whole % 60;
    if (hours > 0) return `${hours} h ${minutes} min`;
    if (minutes > 0) return rest > 0 ? `${minutes} min ${rest} s` : `${minutes} min`;
    return `${rest} s`;
  }

  /** The Save button's words: "Save 53 minutes", "Save 1 h 12 min". */
  function saveLabel(seconds: number): string {
    const minutes = Math.round(seconds / 60);
    if (seconds < 60) return `Save ${Math.round(seconds)} seconds`;
    if (minutes < 60) return `Save ${minutes} minute${minutes === 1 ? "" : "s"}`;
    return `Save ${amount(seconds)}`;
  }

  function percent(seconds: number): number {
    return view && view.seconds > 0 ? (seconds / view.seconds) * 100 : 0;
  }

  function height(level: number | null): number {
    if (level === null) return 6;
    return Math.max(6, Math.min(100, ((level - FLOOR_DB) / -FLOOR_DB) * 100));
  }

  function setStart(seconds: number) {
    start = Math.max(0, Math.min(seconds, end - SHORTEST));
  }

  function setEnd(seconds: number) {
    end = Math.min(view?.seconds ?? 0, Math.max(seconds, start + SHORTEST));
  }

  function at(event: PointerEvent): number {
    if (!track || !view) return 0;
    const box = track.getBoundingClientRect();
    return Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)) * view.seconds;
  }

  /** Pressing on the timeline moves whichever handle is closer, then follows the pointer. */
  function press(event: PointerEvent) {
    if (!view) return;
    const seconds = at(event);
    dragging = Math.abs(seconds - start) <= Math.abs(seconds - end) ? "start" : "end";
    track?.setPointerCapture(event.pointerId);
    move(event);
  }

  function move(event: PointerEvent) {
    if (dragging === "start") setStart(at(event));
    else if (dragging === "end") setEnd(at(event));
  }

  function release() {
    dragging = null;
  }

  /** Arrow keys move a second, with Shift ten; Page Up and Down a minute; Home and End the ends. */
  function key(which: "start" | "end", event: KeyboardEvent) {
    const set = which === "start" ? setStart : setEnd;
    const now = which === "start" ? start : end;
    const step = event.shiftKey ? 10 : 1;
    const moves: Record<string, number> = {
      ArrowLeft: now - step,
      ArrowDown: now - step,
      ArrowRight: now + step,
      ArrowUp: now + step,
      PageDown: now - 60,
      PageUp: now + 60,
      Home: 0,
      End: view?.seconds ?? 0,
    };
    if (event.key in moves) {
      event.preventDefault();
      set(moves[event.key]);
    }
  }

  async function run(command: string, args: Record<string, unknown> = {}) {
    error = "";
    try {
      await invoke(command, args);
    } catch (caught) {
      error = String(caught);
    }
  }

  async function listenFrom(seconds: number) {
    await run("listen_timeline", { fromSeconds: Math.max(0, seconds) });
    poll();
  }

  async function pause() {
    await run("toggle_playback");
    poll();
  }

  async function poll() {
    const state = await invoke<Playback>("playback_state");
    // The timeline's player is the one without a clip.
    const ours = state.path === null && (state.playing || state.positionSeconds > 0);
    playing = ours && state.playing;
    playhead = ours ? state.positionSeconds : null;
  }

  async function save() {
    await run("save_timeline", { startSeconds: start, endSeconds: end });
    if (!error) ondone();
  }

  function cancel() {
    invoke("close_timeline");
    ondone();
  }

  onMount(() => {
    let generation = 0;
    const stopLevels = listen<Levels>("timeline-levels", ({ payload }) => {
      if (payload.generation !== generation) return;
      const next = levels.slice();
      for (const [bar, level] of payload.bars) next[bar] = level;
      levels = next;
    });
    const stopClosed = listen("timeline-closed", () => ondone());
    invoke<View>("open_timeline")
      .then((opened) => {
        generation = opened.generation;
        view = opened;
        levels = Array(opened.bars).fill(null);
        // Starts on the last 15 minutes, like the quick save; drag from there.
        end = opened.seconds;
        start = Math.max(0, opened.seconds - 15 * 60);
      })
      .catch((caught) => (error = String(caught)));
    const timer = setInterval(() => {
      if (playing) poll();
    }, 200);
    return () => {
      clearInterval(timer);
      stopLevels.then((unlisten) => unlisten());
      stopClosed.then((unlisten) => unlisten());
      invoke("close_timeline");
    };
  });
</script>

<div class="timeline">
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if view}
    <p class="hint">
      Everything Hindsight held at {clock(view.seconds)}: {amount(view.seconds)}. Recording carries on
      meanwhile; anything newer isn't in here.
    </p>

    <div
      class="track"
      bind:this={track}
      onpointerdown={press}
      onpointermove={move}
      onpointerup={release}
      onpointercancel={release}
      role="presentation"
    >
      <!-- One unit per bar, stretched to the track's width: any number of bars fits any width. -->
      <svg class="bars" viewBox="0 0 {levels.length} 100" preserveAspectRatio="none" aria-hidden="true">
        {#each levels as level, index (index)}
          {@const middle = ((index + 0.5) / levels.length) * view.seconds}
          {@const tall = height(level)}
          <rect
            class:chosen={middle >= start && middle <= end}
            class:pending={level === null}
            x={index + 0.15}
            y={(100 - tall) / 2}
            width="0.7"
            height={tall}
          />
        {/each}
      </svg>
      <div class="selection" aria-hidden="true" style:left="{percent(start)}%" style:width="{percent(length)}%"></div>
      {#if playhead !== null}
        <div class="playhead" aria-hidden="true" style:left="{percent(playhead)}%"></div>
      {/if}
      <div
        class="handle"
        style:left="{percent(start)}%"
        role="slider"
        tabindex="0"
        aria-label="Start"
        aria-valuemin={0}
        aria-valuemax={view.seconds}
        aria-valuenow={Math.round(start)}
        aria-valuetext={clock(start)}
        onkeydown={(event) => key("start", event)}
      ></div>
      <div
        class="handle"
        style:left="{percent(end)}%"
        role="slider"
        tabindex="0"
        aria-label="End"
        aria-valuemin={0}
        aria-valuemax={view.seconds}
        aria-valuenow={Math.round(end)}
        aria-valuetext={clock(end)}
        onkeydown={(event) => key("end", event)}
      ></div>
    </div>
    <div class="ruler" aria-hidden="true">
      <span>{clock(0)}</span>
      <span>{clock(view.seconds)}</span>
    </div>

    <div class="ends">
      <div class="end">
        <span class="label">Start</span>
        <time>{clock(start)}</time>
        <button aria-label="Start 5 seconds earlier" onclick={() => setStart(start - 5)}>−5 s</button>
        <button aria-label="Start 5 seconds later" onclick={() => setStart(start + 5)}>+5 s</button>
      </div>
      <div class="end">
        <span class="label">End</span>
        <time>{clock(end)}</time>
        <button aria-label="End 5 seconds earlier" onclick={() => setEnd(end - 5)}>−5 s</button>
        <button aria-label="End 5 seconds later" onclick={() => setEnd(end + 5)}>+5 s</button>
      </div>
    </div>

    <div class="buttons">
      {#if playing}
        <button onclick={pause}>❙❙ Pause</button>
      {:else}
        <button onclick={() => listenFrom(start)}>▶ Listen from the start</button>
        <button onclick={() => listenFrom(end - 5)}>▶ The last 5 seconds</button>
      {/if}
      <span class="hint">{amount(length)} chosen</span>
    </div>
    <p class="hint">
      Drag the start and the end, or press on the timeline to move the closer one. With a handle
      focused, the arrow keys move it a second at a time, ten with Shift.
    </p>

    <div class="actions">
      <button onclick={cancel}>Cancel</button>
      <button class="primary" onclick={save}>{saveLabel(length)}</button>
    </div>
  {:else if !error}
    <p class="hint">Freezing what Hindsight holds…</p>
  {:else}
    <div class="actions">
      <button onclick={ondone}>Back</button>
    </div>
  {/if}
</div>

<style>
  .timeline {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  p {
    margin: 0;
  }

  .hint {
    color: var(--muted);
  }

  .error {
    color: var(--text);
  }

  .track {
    position: relative;
    height: 96px;
    padding: var(--space-2) 0;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: ew-resize;
    touch-action: none;
  }

  .bars {
    position: absolute;
    inset: var(--space-2) var(--space-1);
    width: calc(100% - 2 * var(--space-1));
    height: calc(100% - 2 * var(--space-2));
  }

  .bars rect {
    fill: var(--muted);
    opacity: 0.55;
  }

  /* The chosen stretch is a state, so it takes the line color: pastel pink can't reach 3:1 on
     white. */
  .bars rect.chosen {
    fill: var(--accent-edge);
    opacity: 1;
  }

  .bars rect.pending {
    opacity: 0.25;
  }

  .selection {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    pointer-events: none;
  }

  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    margin-left: -1px;
    background: var(--text);
    pointer-events: none;
  }

  .handle {
    position: absolute;
    top: -4px;
    bottom: -4px;
    width: 12px;
    margin-left: -6px;
    border-radius: var(--radius-sm);
    background: linear-gradient(var(--accent-edge), var(--accent-edge)) center / 2px 100% no-repeat;
    cursor: ew-resize;
  }

  .handle::after {
    content: "";
    position: absolute;
    top: 50%;
    left: 50%;
    width: 10px;
    height: 22px;
    transform: translate(-50%, -50%);
    background: var(--accent);
    border: 2px solid var(--accent-edge);
    border-radius: 4px;
  }

  .ruler {
    display: flex;
    justify-content: space-between;
    color: var(--muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .ends {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .end {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .label {
    color: var(--muted);
  }

  time {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }

  .buttons,
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }

  .actions {
    justify-content: flex-end;
  }

  button {
    padding: var(--space-1) var(--space-3);
    background: var(--elevated);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font: inherit;
  }

  .end button {
    padding: 2px var(--space-2);
    font-size: 12px;
  }

  button.primary {
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
  }
</style>
