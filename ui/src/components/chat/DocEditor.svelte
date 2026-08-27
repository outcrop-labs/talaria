<script lang="ts">
  // The editor half of PlanDoc.svelte — mounts once the plan's doc artifact id
  // is known.
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Maximize2, Minimize2 } from '@lucide/svelte'
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
  import { pushToast } from '@/lib/toast.svelte'
  import { p } from '@/router'

  let { id, planId, syncSignal = 0 }: { id: string; planId: string; syncSignal?: number } = $props()

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

  const save = async () => {
    const body = editorRef?.getMarkdown() ?? artifact?.body ?? ''
    try {
      await saveArtifact(id, { body })
    } catch (e) {
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    void qc.invalidateQueries({ queryKey: ['artifact', id] })
  }

  // The plan's agent rewrites the document from the conversation so far.
  const sync = async () => {
    syncing = true
    syncErr = null
    try {
      await postJson(`/api/plan/${planId}/doc`, {})
      await qc.invalidateQueries({ queryKey: ['artifact', id] })
      syncNonce += 1
    } catch (e) {
      syncErr = errorMessage(e)
    } finally {
      syncing = false
    }
  }

  // The document builds as you talk: every landed agent turn triggers a sync.
  // Unsaved manual edits are flushed first so the rewrite starts from them
  // instead of clobbering them. Signal 0 is mount, not a turn.
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
    <div class="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Plan document</span>
      <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{artifact.title}</span>
      <Button size="sm" variant="outline" onclick={() => void sync()} disabled={syncing} title="The agent rewrites the document from the conversation so far">
        {syncing ? 'Syncing' : 'Sync from chat'}
      </Button>
      <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Auto-saves</span>
      <a href={`${p('/artifacts')}?a=${id}`} class="shrink-0 text-[11px] text-accent hover:underline" title="Open in Files">
        Open ↗
      </a>
      <Button variant="ghost" size="sm" class="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onclick={() => (fullscreen = !fullscreen)}>
        {#if fullscreen}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
      </Button>
    </div>
    {#if syncErr}<div class="border-b border-line-subtle px-4 py-1.5 text-xs" style:color="var(--theme-danger)">{syncErr}</div>{/if}
    <div class="relative min-h-0 flex-1 overflow-y-auto">
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
