<script lang="ts">
  import { shell, updateLater, updateNow } from '../lib/state.svelte'

  // The update chip on the launcher. Rust decides whether there is one (a
  // newer Talaria Desktop that nobody put off, src-tauri/src/update.rs) and
  // gives every view the same offer: this one, and the chip it puts into each
  // instance page (`chip_js`, the same layout). Later takes it down
  // everywhere for a day. Where this copy cannot update itself (a Linux
  // package, an .msi install), "How?" says how instead of offering Update.
  // One row, plus a line only while installing, after a failure, or for How?.
  const offer = $derived(shell.update)
  let how = $state(false)
  const line = $derived(
    offer ? (offer.error ?? (offer.busy ? 'Updating. Talaria restarts when it is done.' : how ? offer.how : null)) : null,
  )
</script>

{#if offer}
  <div
    class="fixed bottom-4 right-4 z-40 max-w-[min(340px,calc(100vw-2rem))] rounded-md border border-hairline bg-panel py-1.5 pl-2.5 pr-1.5 text-left text-xs leading-snug text-readout shadow-[0_4px_16px_rgba(0,0,0,0.45)]"
    role="status"
    aria-live="polite"
    data-update-chip={offer.version}
  >
    <div class="flex items-center gap-1.5">
      <p class="min-w-0 flex-1 truncate font-semibold" title="Talaria Desktop {offer.version} is available">
        Talaria Desktop {offer.version} is available
      </p>
      {#if offer.install !== 'updater'}
        <button
          type="button"
          class="shrink-0 rounded px-2 py-0.5 text-[11px] font-semibold text-muted hover:text-readout"
          aria-expanded={how}
          onclick={() => (how = !how)}
        >
          {how ? 'Hide' : 'How?'}
        </button>
      {/if}
      <button
        type="button"
        class="shrink-0 rounded border border-hairline-strong px-2 py-0.5 text-[11px] font-semibold text-readout hover:bg-hover disabled:opacity-50"
        disabled={offer.busy}
        onclick={() => void updateLater()}
      >
        Later
      </button>
      {#if offer.install === 'updater'}
        <button
          type="button"
          class="shrink-0 rounded bg-gold px-2 py-0.5 text-[11px] font-semibold text-ground hover:brightness-110 disabled:opacity-50"
          title="Downloads it and restarts Talaria"
          disabled={offer.busy}
          onclick={() => void updateNow()}
        >
          {offer.busy ? 'Updating' : 'Update'}
        </button>
      {/if}
    </div>
    {#if line}
      <p class="mt-1 mr-1 break-words select-text {offer.error ? 'text-danger' : 'text-muted'}">{line}</p>
    {/if}
  </div>
{/if}
