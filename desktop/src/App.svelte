<script lang="ts">
  import { getCurrentWebview } from '@tauri-apps/api/webview'
  import { onMount } from 'svelte'
  import AddInstanceDialog from './components/AddInstanceDialog.svelte'
  import BrowseBar from './components/BrowseBar.svelte'
  import RemoveDialog from './components/RemoveDialog.svelte'
  import Titlebar from './components/Titlebar.svelte'
  import UpdateChip from './components/UpdateChip.svelte'
  import Welcome from './components/Welcome.svelte'
  import { quit } from './lib/instances'
  import { boot, shell } from './lib/state.svelte'

  // One app, two local surfaces (TALA-83). This same document runs in the
  // launcher webview (`main`) and in the browse bar (`browsebar`); WHICH
  // webview it is decides what it renders — not the shell's view, which both
  // mirror. The launcher keeps rendering Welcome even while browse is on
  // screen (it is hidden), so the moment browse leaves, the launcher is
  // already what it should be, already painted.
  //
  // The bar's boot skips the launcher's reopen (boot's `asBar`): the bar
  // never reopens an instance over the view it is in.
  const bar = getCurrentWebview().label === 'browsebar'

  // Everything behind a modal is `inert`: no Tab stops, no clicks.
  const modal = $derived(shell.adding || shell.confirmRemove !== null)

  onMount(() => {
    void boot(bar)
    // Quit from the launcher too: closing the window only hides Talaria on
    // a desktop that can show it in the tray or the menu bar. Cmd+Q on macOS
    // is the app menu's own.
    const mac = /Mac|iPhone|iPad/.test(navigator.userAgent)
    const onKey = (e: KeyboardEvent) => {
      if (!mac && e.ctrlKey && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'q') {
        e.preventDefault()
        void quit()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  })
</script>

{#if bar}
  <!-- The browse bar: one 44-px strip, no titlebar of its own — the bar IS
       the top chrome of an undecorated window in browse view. -->
  <div class="h-full">
    <BrowseBar />
  </div>
{:else}
  <div class="flex h-full flex-col" inert={modal}>
    <Titlebar mode={shell.titlebar} />
    <div class="min-h-0 flex-1">
      <Welcome />
    </div>
    <UpdateChip />
  </div>
{/if}

{#if shell.adding}
  <AddInstanceDialog onclose={() => (shell.adding = false)} />
{/if}
{#if shell.confirmRemove}
  <RemoveDialog id={shell.confirmRemove} />
{/if}
