<script lang="ts">
  import DiffView from '@/components/fleet/DiffView.svelte'
  import { diffLines, type DiffLine } from '@/components/fleet/line-diff'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { useAgents } from '@/lib/agents'
  import { summarize } from '@/lib/change-summary'
  import { getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { HISTORY_PAGE, useRevisions, type HistoryKind, type Revision } from '@/lib/revisions.svelte'

  /** VERSION HISTORY for a document — a KB doc, a KB space's landing page, or
   *  a file. Every save is a revision, agents' included: who saved it (an
   *  agent by its name), when, what it changed against the version before it,
   *  the version itself, and Restore. This rail is where a document's changes
   *  are reviewed; nothing announces them over the document. */
  let {
    kind = 'kb-doc',
    id,
    format = 'markdown',
    canRestore = true,
    onRestore,
  }: {
    kind?: HistoryKind
    id: string
    /** How a version previews: rendered markdown, or its raw text (a sheet's
     *  grid, a page's HTML). */
    format?: 'markdown' | 'text'
    /** False for a viewer who may read the history but not edit the
     *  document (the server's `canEdit`): no Restore to be refused. */
    canRestore?: boolean
    /** Rejects with the reason when the restore did not land — the dialog
     *  stays open and says so. */
    onRestore: (content: string) => Promise<void>
  } = $props()

  // Shared with the read pane's follower (same key), so a revision it saw
  // land is already listed here. A failed read is its own row — "No saved
  // revisions yet" is a claim about the document's past and only a real
  // 200-with-[] may make it.
  const history = useRevisions(
    () => kind,
    () => id,
  )

  // Agent saves are stamped with the agent's id; say its name. A person's
  // stamp (their email, else name) is already readable.
  const agentsQuery = useAgents()
  const agentNames = $derived(new Map((agentsQuery.data?.agents ?? []).map((a) => [a.id, a.label])))
  const author = (by: string | null) => (by ? (agentNames.get(by) ?? by) : 'unknown')

  interface Opened {
    rev: Revision
    content: string
    /** The line diff against the version before it; null when the document
     *  is too large to diff, undefined when no earlier version is listed. */
    diff: DiffLine[] | null | undefined
    summary: string
  }
  let opened = $state<Opened | null>(null)
  let view = $state<'changes' | 'preview'>('changes')
  let openError = $state<unknown>(null)
  let restoreError = $state<unknown>(null)
  let restoring = $state(false)

  const contentOf = (revId: string) =>
    getJson<{ content: string }>(
      `/api/history?kind=${kind}&id=${encodeURIComponent(id)}&rev=${encodeURIComponent(revId)}`,
    ).then((j) => j.content)

  const open = async (revs: Revision[], i: number) => {
    const rev = revs[i]!
    const older = revs[i + 1]
    // The oldest row of a short list is the document's first version — its
    // change is everything in it. Of a full page, older versions exist that
    // the list does not carry, so there is nothing honest to compare with.
    const first = !older && revs.length < HISTORY_PAGE
    openError = null
    restoreError = null
    try {
      const [content, before] = await Promise.all([
        contentOf(rev.id),
        older ? contentOf(older.id) : Promise.resolve(first ? '' : null),
      ])
      opened = {
        rev,
        content,
        diff: before === null ? undefined : diffLines(before, content),
        summary: before === null ? '' : first ? 'first version' : summarize(before, content).text,
      }
      view = before === null ? 'preview' : 'changes'
    } catch (e) {
      openError = e
    }
  }

  const restore = async () => {
    if (!opened) return
    restoreError = null
    restoring = true
    try {
      await onRestore(opened.content)
      opened = null
    } catch (e) {
      restoreError = e
    } finally {
      restoring = false
    }
  }
</script>

<div>
  <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">History</div>
  {#if openError !== null}
    <div transition:slide={{ duration: 150 }}>
      <QueryError variant="inline" class="mb-2 px-2" title="Could not open that version" error={openError} />
    </div>
  {/if}
  <QueryState
    query={history}
    errorTitle={kind === 'artifact' ? 'Could not load this file’s history' : 'Could not load this document’s history'}
    errorVariant="inline"
  >
    {#snippet skeleton()}<SkeletonRows rows={4} class="px-2 py-1" />{/snippet}
    {#snippet empty()}<EmptyState variant="inline" title="No saved revisions yet." />{/snippet}
    {#snippet children(revs)}
      {#each revs as r, i (r.id)}
        <button
          type="button"
          onclick={() => void open(revs, i)}
          class="block w-full rounded-md px-2 py-1.5 text-left text-xs transition-colors dither-fill"
          title={`${new Date(r.createdAt).toLocaleString()} · ${r.createdBy ?? 'unknown'}`}
          data-revision={r.id}
        >
          <div class="text-fg">{i === 0 ? `Latest · ${relativeTime(r.createdAt)}` : relativeTime(r.createdAt)}</div>
          <div class="truncate font-mono text-[10px] tracking-[0.05em] text-muted">{author(r.createdBy)} · {r.size} chars</div>
        </button>
      {/each}
    {/snippet}
  </QueryState>

  <Modal
    open={!!opened}
    onClose={() => (opened = null)}
    title={opened ? `${author(opened.rev.createdBy)} · ${relativeTime(opened.rev.createdAt)}` : 'Version'}
    width="max-w-3xl"
  >
    {#if opened}
      <div class="mb-3 flex items-center gap-2">
        <Segmented
          size="xs"
          options={[{ id: 'changes', label: 'Changes' }, { id: 'preview', label: 'Version' }] as const}
          value={view}
          onChange={(v) => (view = v)}
        />
        <span class="min-w-0 flex-1 truncate font-mono text-[10px] tracking-[0.05em] text-muted" title={opened.rev.createdBy ?? ''}>
          {new Date(opened.rev.createdAt).toLocaleString()}{opened.summary ? ` · ${opened.summary}` : ''}
        </span>
      </div>
      <div class="max-h-[60vh] overflow-y-auto">
        {#if view === 'changes'}
          {#if opened.diff === undefined}
            <p class="text-sm text-muted">Earlier versions are not listed, so there is nothing to compare this one with.</p>
          {:else}
            <DiffView diff={opened.diff} fallback={opened.content} />
          {/if}
        {:else if format === 'markdown'}
          <Markdown children={opened.content} />
        {:else}
          <pre class="whitespace-pre-wrap font-mono text-xs text-fg">{opened.content}</pre>
        {/if}
      </div>
    {/if}
    {#snippet footer()}
      {#if restoreError !== null}
        <QueryError variant="inline" class="mb-2" title="Could not restore that version" error={restoreError} />
      {/if}
      <div class="flex justify-end gap-2">
        <Button variant="outline" size="sm" onclick={() => (opened = null)}>Close</Button>
        {#if canRestore}
          <Button size="sm" disabled={restoring} onclick={() => void restore()}>
            {restoring ? 'Restoring' : 'Restore this version'}
          </Button>
        {/if}
      </div>
    {/snippet}
  </Modal>
</div>
