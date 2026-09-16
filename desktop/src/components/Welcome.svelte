<script lang="ts">
  import type { DitherSource } from '../lib/dither'
  import DitherLayer from './DitherLayer.svelte'
  import InstanceList from './InstanceList.svelte'
  import WingMark from './WingMark.svelte'
  import { shell } from '../lib/state.svelte'

  let { onadd } = $props<{ onadd: () => void }>()

  // The signature ambient field, wearing the same shape the nav rail wears in
  // the product: a soft bloom off the top edge, a heavier foot at the bottom,
  // a whisper of grain everywhere, gently shimmering.
  const ambient: DitherSource[] = [
    { id: 'chrome', kind: 'edge', side: 'top', depth: 96, strength: 0.16 },
    { id: 'grain', kind: 'uniform', strength: 0.04 },
    { id: 'foot', kind: 'edge', side: 'bottom', depth: 128, strength: 0.1 },
  ]
</script>

<main class="relative grid h-full w-full place-items-center overflow-y-auto bg-ground">
  <DitherLayer sources={ambient} shimmer={0.08} organic={0.5} alphaFloor={0.04} maxAlpha={0.16} />
  <div class="relative z-10 w-[24rem] max-w-full p-6 text-center">
    <WingMark class="mx-auto h-20 w-20" />
    <h1 class="mt-5 font-display text-sm font-semibold tracking-[0.18em] text-readout">TALARIA</h1>

    {#if shell.instances.length > 0}
      <p class="mt-2 text-xs text-muted">Switch below, or add another instance.</p>
      <div class="mt-4 overflow-hidden rounded-md border border-hairline bg-panel text-left">
        <InstanceList />
      </div>
      <button
        class="mt-4 w-full rounded bg-gold px-5 py-2 text-xs font-semibold text-ground hover:brightness-110"
        onclick={onadd}
      >
        Add instance
      </button>
    {:else if shell.loaded}
      <p class="mt-2 text-xs text-muted">
        Your instances, one window. Add one to get started — each keeps its own
        signed-in session, and switching is a click.
      </p>
      <button
        class="mt-6 rounded bg-gold px-5 py-2 text-xs font-semibold text-ground hover:brightness-110"
        onclick={onadd}
      >
        Add instance
      </button>
    {:else}
      <p class="mt-6 text-xs text-dim">…</p>
    {/if}

    {#if shell.error}
      <p class="mt-4 rounded border border-danger/40 bg-danger/10 px-3 py-2 text-left text-xs text-danger">
        {shell.error}
      </p>
    {/if}
  </div>
</main>
