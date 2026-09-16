<script lang="ts">
  import { onMount } from 'svelte'
  import AddInstanceDialog from './components/AddInstanceDialog.svelte'
  import Titlebar from './components/Titlebar.svelte'
  import Welcome from './components/Welcome.svelte'
  import { boot } from './lib/state.svelte'
  import { getDesktopSettings, setTitlebarMode, type TitlebarMode } from './lib/instances'

  // The launcher is one full-window view: brand, the instance list, add.
  // When an instance is active, Rust hides this webview entirely — the
  // instance UI is the window, and switching lives inside it (the desktop
  // switcher beside the logo in ui/). This screen reappears on show_welcome.
  let adding = $state(false)
  let titlebar = $state<TitlebarMode>('themed')

  onMount(() => {
    void boot()
    void getDesktopSettings().then((s) => (titlebar = s.titlebar))
  })

  const onTitlebar = (mode: TitlebarMode) =>
    setTitlebarMode(mode).then((s) => (titlebar = s.titlebar))
</script>

<div class="flex h-full flex-col">
  <Titlebar mode={titlebar} />
  <div class="min-h-0 flex-1">
    <Welcome onadd={() => (adding = true)} {titlebar} ontitlebar={onTitlebar} />
  </div>
</div>

{#if adding}
  <AddInstanceDialog onclose={() => (adding = false)} />
{/if}
