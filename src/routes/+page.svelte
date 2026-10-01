<script lang="ts">
  import { getVersion } from "@tauri-apps/api/app";

  let version = $state("");
  getVersion().then((value) => (version = value));
</script>

<main>
  <h1>Hindsight</h1>
  <p class="status">Not recording yet. The recorder arrives in the next update.</p>
  {#if version}
    <p class="version">Version {version}</p>
  {/if}
</main>

<style>
  main {
    display: grid;
    place-content: center;
    gap: var(--space-2);
    height: 100%;
    padding: var(--space-6);
    text-align: center;
  }

  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 600;
  }

  p {
    margin: 0;
  }

  .status {
    color: var(--text);
  }

  .version {
    color: var(--muted);
    font-size: 12px;
  }
</style>
