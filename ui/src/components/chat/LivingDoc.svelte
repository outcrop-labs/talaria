<script lang="ts">
  // THE LIVING DOCUMENT — a real `doc` artifact, side-by-side with the chat, for
  // either surface that has one: a plan or a work session. One per conversation
  // (linked via artifact_links), found-or-created server-side on first open. A
  // plan's is seeded from the agent's plan template when one is bound; a work
  // session's starts empty, because the shape belongs to the work rather than to
  // a project-plan skeleton. Editable on the fly, autosaved, referenceable anywhere in the app.
  import QueryError from '@/components/ui/QueryError.svelte'
  import DocEditor from './DocEditor.svelte'
  import PlanDocSkeleton from './PlanDocSkeleton.svelte'
  import { getJson } from '@/lib/fetch-json'

  let {
    conversationId,
    syncSignal = 0,
    onDocId,
  }: {
    conversationId: string
    planTitle?: string | null
    syncSignal?: number
    /** The artifact this document resolved to. Reported outward so a surface
     *  can hang things off the same document — the Work view's comments, for
     *  one — without repeating the find-or-create lookup. */
    onDocId?: (id: string | null) => void
  } = $props()

  let docId = $state<string | null>(null)
  // `r.ok ? r.json() : null` folded every failure into the same `null` the
  // pre-fetch state uses, and the render below turns `null` into a skeleton —
  // so a 500 on this lookup shimmered a document outline for ever, silently.
  let error = $state<unknown>(null)
  let reload = $state(0)

  $effect(() => {
    void reload // re-run the lookup when Retry bumps it
    docId = null
    onDocId?.(null)
    error = null
    let cancelled = false
    void getJson<{ artifact: { id: string } }>(`/api/conversations/${conversationId}/doc`)
      .then((j) => {
        if (!cancelled) {
          docId = j.artifact.id
          onDocId?.(docId)
        }
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
      title="Could not open this document"
      {error}
      onRetry={() => (reload += 1)}
    />
  {:else if docId}
    <DocEditor id={docId} {conversationId} {syncSignal} />
  {:else}
    <PlanDocSkeleton />
  {/if}
</div>
