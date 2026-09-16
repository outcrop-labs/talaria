<script lang="ts">
  import { onMount } from 'svelte'
  import AddInstanceDialog from './components/AddInstanceDialog.svelte'
  import Welcome from './components/Welcome.svelte'
  import { boot } from './lib/state.svelte'

  // The launcher is one full-window view: brand, the instance list, add.
  // When an instance is active, Rust hides this webview entirely — the
  // instance UI is the window, and switching lives inside it (the desktop
  // switcher beside the logo in ui/). This screen reappears on show_welcome.
  let adding = $state(false)

  onMount(() => void boot())
</script>

<Welcome onadd={() => (adding = true)} />

{#if adding}
  <AddInstanceDialog onclose={() => (adding = false)} />
{/if}
