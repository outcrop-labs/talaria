<script lang="ts">
  import { shell, updateLater, updateNow } from '../lib/state.svelte'

  // The update chip on the launcher. Rust decides whether there is one (a
  // newer Talaria Desktop that nobody put off, src-tauri/src/update.rs) and
  // gives every view the same offer: this one, and the chip it puts into each
  // instance page (`chip_js`, the same row). Later takes it down everywhere
  // for a day. Where this copy cannot update itself (a Linux package, an .msi
  // install), "How?" says how instead of offering Update. "–" makes it a
  // small pill at any time, an install included: the pill keeps the
  // progress, says when an install failed, and nothing is cancelled.
  const offer = $derived(shell.update)
  let how = $state(false)
  let small = $state(false)
  const pct = $derived(offer?.busy && offer.progress != null ? ` ${offer.progress}%` : '')
  const line = $derived(offer ? (offer.error ?? (how && !offer.busy ? offer.how : null)) : null)
</script>

{#if offer}
  <div
    class="fixed bottom-4 right-4 z-40 max-w-[min(420px,calc(100vw-2rem))] rounded-md border border-hairline bg-panel text-left text-xs leading-snug text-readout shadow-[0_4px_16px_rgba(0,0,0,0.45)]"
    role="status"
    aria-live="polite"
    data-update-chip={offer.version}
    data-state={small ? 'collapsed' : offer.busy ? 'busy' : offer.error ? 'error' : 'offer'}
  >
    {#if small}
      <button
        type="button"
        class="block rounded-md px-2.5 py-1 text-[11px] font-semibold {offer.error ? 'text-danger' : 'text-gold'}"
        title={offer.error
          ? `${offer.error} Show`
          : `Talaria Desktop ${offer.version}${offer.busy ? ': updating' : ' is available'}. Show`}
        onclick={() => (small = false)}
      >
        {offer.error ? '↑ Update failed' : offer.busy ? `↑ Updating${pct}` : `↑ ${offer.version}`}
      </button>
    {:else}
      <div class="flex h-[26px] items-center gap-1.5 pl-2.5 pr-[3px]">
        <p
          class="min-w-0 flex-1 truncate font-semibold"
          title={offer.busy ? 'Talaria restarts when it is done.' : `Talaria Desktop ${offer.version} is available`}
        >
          {offer.busy ? `Updating Talaria Desktop ${offer.version}${pct ? ` ·${pct}` : ''}` : `Talaria Desktop ${offer.version} is available`}
        </p>
        {#if !offer.busy}
          {#if offer.install !== 'updater'}
            <button
              type="button"
              class="shrink-0 rounded px-2 py-px text-[11px] font-semibold text-muted hover:text-readout"
              aria-expanded={how}
              onclick={() => (how = !how)}
            >
              {how ? 'Hide' : 'How?'}
            </button>
          {/if}
          <button
            type="button"
            class="shrink-0 rounded border border-hairline-strong px-2 py-px text-[11px] font-semibold text-readout hover:bg-hover"
            onclick={() => void updateLater()}
          >
            Later
          </button>
          {#if offer.install === 'updater'}
            <button
              type="button"
              class="shrink-0 rounded bg-gold px-2 py-px text-[11px] font-semibold text-ground hover:brightness-110"
              title="Downloads it and restarts Talaria"
              onclick={() => void updateNow()}
            >
              Update
            </button>
          {/if}
        {/if}
        <button
          type="button"
          class="shrink-0 rounded px-2 py-px text-[11px] font-semibold text-muted hover:text-readout"
          title="Make this small"
          aria-label="Make this small"
          onclick={() => (small = true)}
        >
          –
        </button>
      </div>
      {#if line}
        <p class="max-w-[420px] px-2.5 pb-1.5 break-words select-text {offer.error ? 'text-danger' : 'text-muted'}">{line}</p>
      {/if}
    {/if}
  </div>
{/if}
