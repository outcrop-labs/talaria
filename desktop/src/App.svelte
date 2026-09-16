<script lang="ts">
  import { onMount } from 'svelte'
  import AddInstanceDialog from './components/AddInstanceDialog.svelte'
  import InstanceList from './components/InstanceList.svelte'
  import Welcome from './components/Welcome.svelte'
  import { boot, shell, welcome } from './lib/state.svelte'

  let adding = $state(false)

  onMount(() => void boot())

  // One webview, two shapes: full-window welcome (no instance active) and the
  // sidebar Rust squeezes it into when one is. The mode is derived from the
  // launcher's own state — the same state that drove activate/showWelcome —
  // never inferred from viewport width: logical-unit → CSS-pixel conversion
  // under WebKitGTK scale factors is exactly the kind of arithmetic that
  // strands a 240px strip showing the full-window layout.
  const sidebar = $derived(shell.loaded && shell.activeId !== null)
</script>

{#if sidebar}
  <aside class="flex h-full w-full flex-col border-r border-hairline bg-panel">
    <header class="px-4 pt-4 pb-3">
      <span class="font-semibold tracking-[0.18em] text-muted text-xs">TALARIA</span>
    </header>
    <InstanceList />
    <footer class="border-t border-hairline p-2">
      <button
        class="w-full rounded px-2 py-1.5 text-left text-xs text-muted hover:bg-hover hover:text-readout"
        onclick={() => void welcome()}
      >
        ⌂ Home
      </button>
      <button
        class="mt-1 w-full rounded px-2 py-1.5 text-left text-xs font-medium text-readout hover:bg-hover"
        onclick={() => (adding = true)}
      >
        + Add instance
      </button>
    </footer>
  </aside>
{:else}
  <Welcome onadd={() => (adding = true)} />
{/if}

{#if adding}
  <AddInstanceDialog onclose={() => (adding = false)} />
{/if}
