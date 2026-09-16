<script lang="ts">
  import { shell } from '../lib/state.svelte'

  let { onadd } = $props<{ onadd: () => void }>()
</script>

<main class="grid h-full place-items-center bg-ground">
  <div class="w-[24rem] max-w-full text-center">
    <svg viewBox="0 0 512 512" class="mx-auto h-20 w-20" aria-hidden="true">
      <g fill="#C8B46C">
        <path
          d="M256 96 L392 216 Q400 224 392 236 L384 248 Q376 258 364 250 L256 168 L148 250 Q136 258 128 248 L120 236 Q112 224 120 216 Z"
        />
        <path
          d="M256 196 L392 316 Q400 324 392 336 L384 348 Q376 358 364 350 L256 268 L148 350 Q136 358 128 348 L120 336 Q112 324 120 316 Z"
        />
        <rect x="236" y="330" width="40" height="96" rx="12" />
      </g>
    </svg>
    <h1 class="mt-5 text-sm font-semibold tracking-[0.18em] text-readout">TALARIA</h1>
    <p class="mt-2 text-xs text-muted">
      Your instances, one window. Add one to get started — each keeps its own
      signed-in session, and switching is a click.
    </p>
    {#if shell.loaded}
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
      <p class="mt-4 rounded border border-danger/40 bg-danger/10 px-3 py-2 text-xs text-danger">
        {shell.error}
      </p>
    {/if}
  </div>
</main>
