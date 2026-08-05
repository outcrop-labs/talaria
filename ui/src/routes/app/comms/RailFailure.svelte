<script lang="ts">
  import QueryError from '@/components/ui/QueryError.svelte'

  // A rail section whose read FAILED. It sits exactly where the list would have
  // been, so a broken section can never be mistaken for an empty one — "No agents
  // yet. Hire on /agents." over a 500 tells an owner to re-hire a fleet that is
  // still there. `<li>` because Section renders its children inside a `<ul>`.
  let { error, title, onRetry }: { error: unknown; title: string; onRetry: () => void } = $props()
</script>

<li class="px-2 py-1.5">
  <QueryError variant="inline" {error} {title} {onRetry} />
</li>
