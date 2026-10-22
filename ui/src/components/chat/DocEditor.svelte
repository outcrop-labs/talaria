<script lang="ts">
  // The editor half of LivingDoc.svelte — mounts once the conversation doc id
  // is known. A PLAN's document: only plan sessions have one now (a work
  // session's pane is its files — `components/work/FilePane`).
  import { tick } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Maximize2, MessageSquareText, Minimize2 } from '@lucide/svelte'
  import ArtifactComments from '@/components/work/ArtifactComments.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import GeneratingOverlay from '@/components/ui/GeneratingOverlay.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import type { RichEditorHandle } from '@/components/ui/rich-editor'
  import PlanDocSkeleton from './PlanDocSkeleton.svelte'
  import { saveArtifact, useArtifact } from '@/lib/artifacts'
  import { cn } from '@/lib/cn'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { onUserEvent } from '@/lib/user-events.svelte'
  import { p } from '@/router'

  let { id, conversationId, syncSignal = 0 }: { id: string; conversationId: string; syncSignal?: number } = $props()

  const qc = useQueryClient()
  const artifactQuery = useArtifact(() => id)
  const artifact = $derived(artifactQuery.data)
  let editorRef = $state<RichEditorHandle | null>(null)
  let syncing = $state(false)
  let syncErr = $state<string | null>(null)
  // The editor is uncontrolled after mount — bump to remount on an agent sync.
  let syncNonce = $state(0)
  // Fullscreen (Esc exits) — same affordance as the artifact/KB editors.
  let fullscreen = $state(false)
  // Comments swap in over the editor rather than beside it: the pane is a
  // column of a split stage, too narrow to split again. The editor stays
  // MOUNTED underneath (hidden), so its caret and scroll survive the visit.
  let showComments = $state(false)
  // The box around the editor. The scroll container itself is the editor's
  // own content box inside it (RichEditor `fill`), found when it is needed.
  let editorBox = $state<HTMLDivElement | null>(null)
  // WHAT THE EDITOR IS SHOWING, as far as this component knows: the body it
  // was seeded with, or the last body it saved. A sync remounts only when the
  // server's body differs from it — the editor is uncontrolled, so a remount is
  // the only way to show a rewrite, and an unneeded one throws the reader back
  // to the top of the document on every turn.
  let shownBody: string | null = null
  $effect(() => {
    if (shownBody === null && artifact) shownBody = artifact.body
  })

  // TALA-33: the SERVER auto-syncs this document after every landed plan
  // turn and fans a conversation event — this pane no longer waits for its
  // own sync() to refetch. A matching 'conversation' event invalidates the
  // artifact query (the same key sync() invalidates), which re-renders the
  // editor through the artifact data — including landings this tab never
  // observed, and writes from other members' tabs.
  $effect(() => {
    const off = onUserEvent((event) => {
      if (event.type === 'conversation' && event.conversationId === conversationId) {
        void qc.invalidateQueries({ queryKey: ['artifact', id] })
      }
    })
    return off
  })

  const save = async () => {
    const body = editorRef?.getMarkdown() ?? artifact?.body ?? ''
    try {
      await saveArtifact(id, { body })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    shownBody = body
    void qc.invalidateQueries({ queryKey: ['artifact', id] })
  }

  /** The element that actually scrolls: RichEditor's content box. Queried
   *  rather than bound because it belongs to the editor, which is remounted. */
  const scroller = (): HTMLElement | null => editorBox?.querySelector<HTMLElement>('.overflow-y-auto') ?? null

  /** Remount the editor on the body the server now has, and put the reader
   *  back where they were — a rewrite usually changes a paragraph, not the
   *  place somebody was reading. */
  const reseed = async (body: string) => {
    const top = scroller()?.scrollTop ?? 0
    shownBody = body
    syncNonce += 1
    await tick()
    // The editor builds its document on mount; one frame later it has a height
    // to scroll within.
    requestAnimationFrame(() => {
      const el = scroller()
      if (el) el.scrollTop = top
    })
  }

  // The plan's agent rewrites the document from the conversation so far.
  const sync = async () => {
    syncing = true
    syncErr = null
    try {
      await postJson(`/api/conversations/${conversationId}/doc`, {})
      await qc.invalidateQueries({ queryKey: ['artifact', id] })
      // The server's recency guard often makes this a no-op (its own post-turn
      // sync got there first, or nothing new was said): same body, no remount.
      const fresh = qc.getQueryData<{ body: string } | null>(['artifact', id])?.body
      if (fresh !== undefined && fresh !== shownBody) await reseed(fresh)
    } catch (e) {
      syncErr = errorMessage(e)
    } finally {
      syncing = false
    }
  }

  // The document builds as you talk: every landed agent turn triggers a sync.
  // Unsaved manual edits are flushed first so the rewrite starts from them
  // instead of clobbering them. Signal 0 is mount, not a turn.
  // svelte-ignore state_referenced_locally -- reason: the last-seen-signal guard's baseline; the effect below updates it on every real bump
  let lastSignal = syncSignal
  $effect(() => {
    if (syncSignal === lastSignal) return
    lastSignal = syncSignal
    void (async () => {
      const md = editorRef?.getMarkdown()
      if (md !== undefined && md !== artifact?.body) await save().catch(() => {})
      await sync()
    })()
  })
</script>

<svelte:window
  onkeydown={(e) => {
    if (fullscreen && e.key === 'Escape') fullscreen = false
  }}
/>

<!-- Three answers again: undefined = loading, null = a real 404 (the document
     was deleted out from under the plan), isError = the read failed. -->
{#if artifactQuery.isError && artifact === undefined}
  <QueryError
    class="p-6"
    variant="compact"
    title="Could not load this plan’s document"
    error={artifactQuery.error}
    onRetry={() => void artifactQuery.refetch()}
  />
{:else if artifact === null}
  <EmptyState
    variant="compact"
    icon="⧉"
    title="This plan’s document is gone"
    hint="It may have been deleted, or it is no longer shared with you."
  />
{:else if !artifact}
  <PlanDocSkeleton />
{:else}
  <div class={cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'flex-1')}>
    <div class="flex h-11 shrink-0 items-center gap-2 border-b border-line-subtle pl-4 pr-2">
      <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Plan</span>
      <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg" title={artifact.title}>{artifact.title}</span>
      <Button size="xs" variant="outline" class="shrink-0" onclick={() => void sync()} disabled={syncing} title="The agent rewrites the document from the conversation so far">
        {syncing ? 'Syncing' : 'Sync from chat'}
      </Button>
      <span class="hidden shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim xl:inline">Auto-saves</span>
      <span class="flex shrink-0 items-center">
        <IconButton
          size="sm"
          title={showComments ? 'Back to the plan' : 'Comments'}
          active={showComments}
          onclick={() => (showComments = !showComments)}
        >
          <MessageSquareText size={14} />
        </IconButton>
        <a href={`${p('/artifacts')}?a=${id}`} class="px-1.5 text-[11px] text-accent hover:underline" title="Open in Files">
          Open ↗
        </a>
        <IconButton size="sm" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onclick={() => (fullscreen = !fullscreen)}>
          {#if fullscreen}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
        </IconButton>
      </span>
    </div>
    {#if syncErr}<div class="border-b border-line-subtle px-4 py-1.5 text-xs" style:color="var(--theme-danger)">{syncErr}</div>{/if}
    {#if showComments}
      <div class="flex min-h-0 flex-1">
        <ArtifactComments artifactId={id} onClose={() => (showComments = false)} />
      </div>
    {/if}
    <!-- A flex column, NOT a scroll box: the editor (`fill`) owns the scroll,
         under its own toolbar, and needs a bounded parent to own it in. Two
         nested scroll boxes was how a long plan stopped scrolling at all. -->
    <div bind:this={editorBox} class={cn('relative flex min-h-0 flex-1 flex-col', showComments && 'hidden')}>
      {#if syncing}<GeneratingOverlay site="chat/doc-resync" label="Rewriting the plan document from the conversation" />{/if}
      {#key `${id}:${syncNonce}`}
        <RichEditor
          bind:this={editorRef}
          value={artifact.body}
          slash
          prose
          autosave
          onSave={() => void save()}
          placeholder="The plan takes shape here: outline goals, scope, and decisions. Draft tickets from it when ready."
          fill
          class="min-w-0 flex-1"
        />
      {/key}
    </div>
  </div>
{/if}
