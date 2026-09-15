<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { RotateCcw, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import DiffView from '@/components/fleet/DiffView.svelte'
  import { diffLines, type DiffLine } from '@/components/fleet/line-diff'
  import { getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { refineToAnnounce, snapshotBody, summarize, viewerStamp } from '@/lib/agent-refine'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'

  /** THE AGENT-REFINE NOTICE — visual feedback for the refine nobody saw.
   *
   *  An agent editing a document through its toolkit (`edit_kb_doc`,
   *  `update_document`) writes through the same save route a human's editor
   *  uses, so the change arrives as an ordinary revision snapshot — stamped
   *  with the AGENT's identity, never the signed-in human's. This component
   *  watches those snapshots (light poll of /api/history) and, when the newest
   *  revision's author is not the local viewer, announces the refine: a
   *  diff-with-summary on the document surface, a Load-into-editor review
   *  path, and a dismiss that persists per viewer. The DECISIONS live in
   *  `@/lib/agent-refine` (tested there); this file owns the effects and DOM.
   *
   *  Deliberately NO "in progress" state: a toolkit edit is one request —
   *  there is no honest in-between, and a fake spinner would lie. Detection
   *  is bounded by the poll interval; that bound is the notice's cost. */

  interface Rev {
    id: string
    createdBy: string | null
    createdAt: string
    size: number
  }

  let {
    kind = 'kb-doc',
    id,
    /** What the viewer's editor currently holds (their possibly-unsaved text).
     *  Read as a getter, evaluated when a refine lands. */
    current,
    /** Pull the revised content into the viewer's editor (unsaved). */
    onLoad,
    /** The signed-in user's identity, as saves stamp it. */
    me,
    /** A refine just landed and was announced — e.g. a read pane refreshing
     *  itself (it holds no buffer, so it can follow the server directly). */
    onAnnounce,
    class: className,
  }: {
    kind?: 'kb-doc' | 'kb-space' | 'artifact'
    id: string
    current?: () => string
    onLoad?: (markdown: string) => void
    me?: { email: string | null; name: string | null } | null
    onAnnounce?: () => void
    class?: string
  } = $props()

  const who = $derived(viewerStamp(me ?? null))

  // Watch the item's revisions. gcTime 0 so a remount re-syncs instead of
  // answering from a stale cache — the notice keys on content, not cache age.
  const historyQuery = createQuery(() => ({
    queryKey: ['agent-refine', kind, id],
    enabled: !!id,
    refetchInterval: 4_000,
    gcTime: 0,
    queryFn: (): Promise<{ revisions: Rev[] }> =>
      getJson(`/api/history?kind=${kind}&id=${encodeURIComponent(id)}`),
  }))
  const revisions = $derived(historyQuery.data?.revisions ?? [])

  // The last revision this VIEWER has been told about — persisted so a remount
  // doesn't re-announce a refine already seen. The key carries the viewer
  // identity, so two people sharing a browser share nothing.
  const seenKey = () => `agent-refine-seen:${kind}:${id}:${who ?? '?'}`
  const seenAt = (): string | null => {
    try {
      return localStorage.getItem(seenKey())
    } catch {
      return null
    }
  }
  const markSeen = (createdAt: string) => {
    try {
      localStorage.setItem(seenKey(), createdAt)
    } catch {
      /* private mode — the notice just re-announces on the next mount */
    }
  }

  let refined = $state<{ at: string; by: string | null } | null>(null)

  // The text the viewer held when the refine landed — the diff's "before".
  // Captured at announce time because the surface can move under the notice
  // (e.g. a read pane auto-refreshing to the refined body) and a diff computed
  // later would compare the refined text against itself and read as no change.
  let beforeText = $state<string | null>(null)

  // The revision's saved content and its summary, fetched AT announce — not
  // on click. The summary rides the headline immediately (TALA-4: "a summary
  // of what changed", not behind a second interaction). The diff is computed
  // here too but stays closed until Show changes — a big doc should not
  // unfold itself unasked.
  let savedText = $state<string | null>(null)
  let diff = $state<DiffLine[] | null>(null)
  let showDiff = $state(false)
  let summaryText = $state<string | null>(null)

  const revUrl = (revId: string) =>
    `/api/history?kind=${kind}&id=${encodeURIComponent(id)}&rev=${encodeURIComponent(revId)}`

  /** Fetch the refined revision, set the summary and the diff. A failed fetch
   *  takes the notice down rather than leaving a claim on screen that nothing
   *  backs. */
  const hydrate = async (revId: string) => {
    try {
      const j = await getJson<{ content: string }>(revUrl(revId))
      const saved = snapshotBody(j.content)
      const before = beforeText ?? current?.() ?? saved
      savedText = saved
      summaryText = summarize(before, saved).text
      diff = diffLines(before, saved)
    } catch {
      refined = null
      markSeen(revId)
    }
  }

  $effect(() => {
    const revs = revisions
    if (refined) return // already announcing this one
    const next = refineToAnnounce(revs, who, seenAt())
    if (!next) return
    beforeText = current?.() ?? null
    refined = { at: next.createdAt, by: next.createdBy }
    void hydrate(next.id)
    onAnnounce?.()
  })

  /** Open the diff panel — content and diff were computed at announce. */
  const show = () => {
    if (savedText === null) return
    showDiff = true
  }
  const hide = () => (showDiff = false)

  /** Load the revised text into the viewer's editor (not saved by this). */
  const load = () => {
    const latest = revisions[0]
    if (!latest) return
    getJson<{ content: string }>(
      `/api/history?kind=${kind}&id=${encodeURIComponent(id)}&rev=${encodeURIComponent(latest.id)}`,
    )
      .then((j) => {
        onLoad?.(snapshotBody(j.content))
        refined = null
        markSeen(latest.createdAt)
      })
      .catch(() => {})
  }

  const dismiss = () => {
    const latest = revisions[0]
    if (latest) markSeen(latest.createdAt)
    refined = null
  }
</script>

{#if refined}
  <div
    transition:slide={{ duration: 150 }}
    class={cn('rounded-lg border border-line border-l-2 border-l-accent bg-panel', className)}
    data-agent-refine="visible"
  >
    <div class="flex items-center gap-2 px-3 pt-2 text-xs text-muted">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent refine</span>
      <span class="min-w-0 flex-1 truncate">
        {refined.by ? `${refined.by} updated this` : 'This document was updated'}
        {relativeTime(refined.at)}{#if summaryText}&nbsp;· {summaryText}{/if}
      </span>
      {#if summaryText === null}
        <span class="text-ink-dim">…</span>
      {/if}
      {#if diff === null}
        <span class="text-ink-dim">changed (diff unavailable)</span>
      {:else if !showDiff}
        <Button variant="outline" size="sm" class="shrink-0" onclick={show}>Show changes</Button>
      {:else}
        <Button variant="outline" size="sm" class="shrink-0" onclick={hide}>Hide changes</Button>
      {/if}
    </div>
    {#if showDiff && diff !== null}
      <div class="max-h-56 overflow-y-auto p-3 pt-2">
        <DiffView {diff} fallback="" />
      </div>
    {/if}
    <div class="flex items-center gap-2 px-3 pb-2 pt-1">
      <Button size="sm" onclick={load}> <RotateCcw size={13} class="mr-1" /> Load into editor </Button>
      <Button variant="ghost" size="sm" class="ml-auto shrink-0" onclick={dismiss}> <X size={13} /> Dismiss </Button>
    </div>
  </div>
{/if}
