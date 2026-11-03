<script lang="ts">
  import { shell, updateLater, updateNow } from '../lib/state.svelte'

  // The update chip on the launcher. Rust decides whether there is one (a
  // newer Talaria Desktop that nobody put off, src-tauri/src/update.rs) and
  // gives every view the same offer: this one, and the chip it puts into each
  // instance page. Later takes it down everywhere for a day. Where this copy
  // cannot update itself (a Linux package, an .msi install), it says how
  // instead of offering Update.
  const offer = $derived(shell.update)
  const line = $derived(
    offer
      ? (offer.error ??
          (offer.busy
            ? 'Updating. Talaria restarts when it is done.'
            : (offer.how ?? 'Update downloads it and restarts Talaria.')))
      : '',
  )
</script>

{#if offer}
  <div
    class="fixed bottom-4 right-4 z-40 w-[min(340px,calc(100vw-2rem))] rounded-md border border-hairline bg-panel px-3 py-2.5 text-left text-[13px] leading-snug text-readout shadow-[0_6px_24px_rgba(0,0,0,0.45)]"
    role="status"
    aria-live="polite"
    data-update-chip={offer.version}
  >
    <p class="font-semibold">Talaria Desktop {offer.version} is available</p>
    <p class="mt-1 break-words select-text {offer.error ? 'text-danger' : 'text-muted'}">{line}</p>
    <div class="mt-2 flex justify-end gap-2">
      <button
        type="button"
        class="rounded border border-hairline-strong px-2.5 py-0.5 text-xs font-semibold text-readout hover:bg-hover disabled:opacity-50"
        disabled={offer.busy}
        onclick={() => void updateLater()}
      >
        Later
      </button>
      {#if offer.install === 'updater'}
        <button
          type="button"
          class="rounded bg-gold px-2.5 py-0.5 text-xs font-semibold text-ground hover:brightness-110 disabled:opacity-50"
          disabled={offer.busy}
          onclick={() => void updateNow()}
        >
          {offer.busy ? 'Updating' : 'Update'}
        </button>
      {/if}
    </div>
  </div>
{/if}
