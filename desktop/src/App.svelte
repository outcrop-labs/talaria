<script lang="ts">
  import { onMount } from 'svelte'
  import AddInstanceDialog from './components/AddInstanceDialog.svelte'
  import RemoveDialog from './components/RemoveDialog.svelte'
  import Titlebar from './components/Titlebar.svelte'
  import Welcome from './components/Welcome.svelte'
  import { boot, shell } from './lib/state.svelte'

  // The launcher is one full-window view: brand, the instance list, add.
  // When an instance is active, Rust hides this webview entirely — the
  // instance UI is the window, and switching lives inside it (the desktop
  // switcher beside the logo in ui/). This screen reappears on show_welcome.
  //
  // Everything behind a modal is `inert`: no Tab stops, no clicks.
  const modal = $derived(shell.adding || shell.confirmRemove !== null)

  onMount(() => {
    void boot()
  })
</script>

<div class="flex h-full flex-col" inert={modal}>
  <Titlebar mode={shell.titlebar} />
  <div class="min-h-0 flex-1">
    <Welcome />
  </div>
</div>

{#if shell.adding}
  <AddInstanceDialog onclose={() => (shell.adding = false)} />
{/if}
{#if shell.confirmRemove}
  <RemoveDialog id={shell.confirmRemove} />
{/if}
