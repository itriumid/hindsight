<script lang="ts">
  import "@itrium/palettes/palettes.css";
  import "../app.css";
  import { onMount } from "svelte";
  import { theme } from "$lib/theme";

  let { children } = $props();

  onMount(() => theme.apply());
</script>

{@render children()}
