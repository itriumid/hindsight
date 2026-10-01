<script lang="ts">
  import { commonConflict, formatHotkey, fromKeyboardEvent, requiredModifiers } from "$lib/hotkey";

  let {
    hotkey,
    failure = null,
    onsave,
  }: {
    hotkey: string | null;
    /** Why the system refused this hotkey, if it did. */
    failure?: string | null;
    /** Resolves when saved; rejects with the reason it couldn't be. */
    onsave: (hotkey: string | null) => Promise<void>;
  } = $props();

  let recording = $state(false);
  let error = $state("");

  function listen(event: KeyboardEvent) {
    event.preventDefault();
    event.stopPropagation();
    const recorded = fromKeyboardEvent(event);
    if (recorded.kind === "incomplete") return;
    stop();
    if (recorded.kind === "hotkey") save(recorded.hotkey);
  }

  function start() {
    error = "";
    recording = true;
    window.addEventListener("keydown", listen, { capture: true });
  }

  function stop() {
    recording = false;
    window.removeEventListener("keydown", listen, { capture: true });
  }

  async function save(value: string | null) {
    try {
      await onsave(value);
      error = "";
    } catch (caught) {
      error = String(caught);
    }
  }

  $effect(() => stop);
</script>

<div class="recorder">
  <button class:recording onclick={() => (recording ? stop() : start())}>
    {#if recording}
      Press a shortcut…
    {:else if hotkey}
      <kbd>{formatHotkey(hotkey)}</kbd>
    {:else}
      Set hotkey
    {/if}
  </button>
  {#if hotkey && !recording}
    <button class="clear" aria-label="Clear hotkey" onclick={() => save(null)}>×</button>
  {/if}
</div>
{#if recording}
  <p class="hint">Include {requiredModifiers()}. Esc cancels.</p>
{:else if error}
  <p class="problem" role="alert">{error}</p>
{:else if failure}
  <p class="problem" role="alert">Not working: {failure}</p>
{:else if commonConflict(hotkey)}
  <p class="problem">{commonConflict(hotkey)}</p>
{/if}

<style>
  .recorder {
    display: inline-flex;
    gap: var(--space-1);
  }

  button {
    padding: var(--space-2) var(--space-3);
    background: var(--elevated);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font: inherit;
  }

  button.recording {
    border-color: var(--accent-edge);
  }

  .clear {
    padding: var(--space-2);
    color: var(--muted);
  }

  kbd {
    font: inherit;
    font-weight: 600;
    letter-spacing: 0.04em;
  }

  .hint,
  .problem {
    flex-basis: 100%;
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  .problem {
    color: var(--text);
    padding-left: var(--space-2);
    border-left: 2px solid var(--accent-edge);
  }
</style>
