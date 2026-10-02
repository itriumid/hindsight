<!-- The first-run screen. Nothing records until "Start recording": nobody should be recorded
     before the person running Hindsight has read what it does. -->
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  let { onstart }: { onstart: () => void } = $props();

  let launchAtLogin = $state(false);
  let folder = $state<string | null>(null);
  let error = $state("");
  let starting = $state(false);

  async function chooseFolder() {
    const picked = await invoke<string | null>("pick_folder");
    if (picked) folder = picked;
  }

  async function start() {
    starting = true;
    error = "";
    try {
      await invoke("finish_welcome", { launchAtLogin, clipsFolder: folder });
      onstart();
    } catch (caught) {
      error = String(caught);
      starting = false;
    }
  }
</script>

<main>
  <h1>Hindsight keeps the last few minutes, so you don't have to.</h1>
  <p class="lead">
    It records what's said near your computer into memory, all the time, and forgets anything
    older than the last few minutes. When someone says something you'll want later, you save it.
  </p>

  <ul class="promises">
    <li><strong>In memory only.</strong> Nothing is written to disk until you save a clip.</li>
    <li><strong>Encrypted while it waits.</strong> With a key that only exists while Hindsight runs.</li>
    <li><strong>Never leaves your computer.</strong> Hindsight makes no network requests.</li>
  </ul>

  <section class="people">
    <h2>Recording people</h2>
    <p>
      In many places, recording a conversation needs the consent of everyone in it. Hindsight
      shows that it's recording in the menu bar, and your system shows its microphone indicator,
      but whether recording is allowed where you are is up to you. Let the people around you know.
    </p>
  </section>

  <section class="options">
    <label class="check">
      <input type="checkbox" bind:checked={launchAtLogin} />
      Start Hindsight when I log in
    </label>
    <div class="folder">
      <span>Clips go to</span>
      <strong title={folder ?? undefined}>{folder ?? "a folder you choose on the first save"}</strong>
      <button onclick={chooseFolder}>{folder ? "Change…" : "Choose now…"}</button>
    </div>
    <p class="hint">You can change both, and the microphone, in Settings anytime.</p>
  </section>

  {#if error}<p class="error">{error}</p>{/if}

  <div class="start">
    <button class="primary" onclick={start} disabled={starting}>Start recording</button>
    <p class="hint">Your system will ask whether Hindsight may use the microphone.</p>
  </div>
</main>

<style>
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
    line-height: 1.25;
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

  .lead {
    font-size: 14px;
  }

  .promises {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: 0;
    padding: var(--space-4);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    list-style: none;
  }

  .options {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .check {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  input[type="checkbox"] {
    accent-color: var(--accent-edge);
  }

  .folder {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }

  .folder strong {
    overflow: hidden;
    max-width: 320px;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hint {
    color: var(--muted);
  }

  .error {
    padding: var(--space-2);
    background: var(--elevated);
    border-left: 2px solid var(--accent-edge);
  }

  .start {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
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

  button.primary {
    padding: var(--space-2) var(--space-4);
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
    font-weight: 600;
  }

  button:disabled {
    opacity: 0.6;
  }
</style>
