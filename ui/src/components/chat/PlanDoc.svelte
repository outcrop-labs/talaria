<script lang="ts">
  // The plan's living document — a real `doc` artifact, side-by-side with the chat.
  // One per plan (linked via artifact_links target_type='plan'); found-or-created
  // server-side on first open, seeded from the agent's plan template when one is
  // bound. Editable on the fly, autosaved, referenceable anywhere in the app.
  import QueryError from '@/components/ui/QueryError.svelte'
  import DocEditor from './DocEditor.svelte'
  import PlanDocSkeleton from './PlanDocSkeleton.svelte'
  import { getJson } from '@/lib/fetch-json'

  let { planId, syncSignal = 0 }: { planId: string; planTitle?: string | null; syncSignal?: number } = $props()

  let docId = $state<string | null>(null)
  // `r.ok ? r.json() : null` folded every failure into the same `null` the
  // pre-fetch state uses, and the render below turns `null` into a skeleton —
  // so a 500 on this lookup shimmered a document outline for ever, silently.
  let error = $state<unknown>(null)
  let reload = $state(0)

  $effect(() => {
    void reload // re-run the lookup when Retry bumps it
    docId = null
    error = null
    let cancelled = false
    void getJson<{ artifact: { id: string } }>(`/api/plans/${planId}/doc`)
      .then((j) => {
        if (!cancelled) docId = j.artifact.id
      })
      .catch((e: unknown) => {
        if (!cancelled) error = e
      })
    return () => {
      cancelled = true
    }
  })
</script>

<div class="flex min-w-0 flex-col border-l border-line-subtle">
  {#if error}
    <QueryError
      class="p-6"
      variant="compact"
      title="Could not open this plan’s document"
      {error}
      onRetry={() => (reload += 1)}
    />
  {:else if docId}
    <DocEditor id={docId} {planId} {syncSignal} />
  {:else}
    <PlanDocSkeleton />
  {/if}
</div>
