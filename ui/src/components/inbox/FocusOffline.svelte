<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'

  let { error, onRetry }: { error: unknown; onRetry: () => void } = $props()
</script>

<section class="grid min-h-[430px] place-items-center text-center">
  <div class="max-w-md">
    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">Queue offline</div>
    <h2 class="mt-2 font-sans text-2xl font-light text-fg">The decision queue could not load.</h2>
    <!-- The server's own reason, not a shrug. Same component the rest of the
         app reports a failed read with, so this page cannot drift into its
         own private vocabulary for "it broke". -->
    <QueryError
      class="mt-3"
      variant="inline"
      {error}
      title="Nothing was read, so nothing here is a count."
      {onRetry}
    />
    <p class="mx-auto mt-3 font-sans text-sm text-muted">
      This is not an empty inbox. Approvals, blocked agents and tickets waiting on you may be queued behind this.
      They stay queued, and nothing was actioned.
    </p>
    <Button class="mt-5" size="sm" variant="outline" onclick={onRetry}>Retry</Button>
  </div>
</section>
