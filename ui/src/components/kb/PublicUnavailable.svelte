<script lang="ts">
  import PublicShell from './PublicShell.svelte'

  // The OTHER failure. All three public routes used to `.catch(() => error)` and
  // render `PublicNotFound` for every status, so a 500 on a share link told a
  // visitor — someone outside the org, who cannot check — that the page does not
  // exist. That is the one message a working link must never produce: the sender
  // gets told their link is broken and reshares or rebuilds the document.
  // A 404 is still "Not found". Everything else says come back, and offers a
  // retry, because everything else is temporary.
  let { onRetry, detail }: { onRetry?: () => void; detail?: string } = $props()
</script>

<PublicShell>
  <div class="py-16 text-center">
    <div class="mb-2 text-2xl font-semibold text-fg">This page isn’t loading right now</div>
    <p class="text-sm text-muted">
      The link is fine; the server didn’t answer. Try again in a moment.
    </p>
    {#if detail}<p class="mt-2 text-xs text-muted">{detail}</p>{/if}
    {#if onRetry}
      <button
        type="button"
        onclick={onRetry}
        class="mt-5 rounded-lg border border-line px-4 py-2 text-sm text-fg transition-colors hover:border-[var(--theme-accent-border)]"
      >
        Try again
      </button>
    {/if}
  </div>
</PublicShell>
