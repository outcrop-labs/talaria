<script lang="ts">
  // THE LIVING DOCUMENT — a real `doc` artifact, side-by-side with the chat, for
  // a PLAN session. One per conversation (linked via artifact_links),
  // found-or-created server-side on first open, seeded from the agent's plan
  // template when one is bound. Editable on the fly, autosaved, referenceable
  // anywhere in the app.
  //
  // A work session has none: its pane is the files it works on, and the doc
  // route answers 404 for a work session that never had one. An older work
  // session's living document is listed among its files instead.
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

<!-- THE HEIGHT CHAIN. Without `h-full min-h-0 flex-1` this root grew to the
     editor's height, so the editor's own scroll box had nothing to scroll
     within and a long plan simply ran off the bottom of the pane. No left
     hairline: the host column draws the divider. -->
<div class="flex h-full min-h-0 min-w-0 flex-1 flex-col">
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
