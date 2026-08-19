<script lang="ts">
  import type { Snippet } from 'svelte'
  import { createQuery } from '@tanstack/svelte-query'
  import { Check, History, RotateCcw, Sparkles, Square, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import type { RichEditorHandle } from '@/components/ui/rich-editor'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { getList } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { streamMuse, type MuseKind } from '@/lib/muse.svelte'
  import { fly, listStagger, slide, GROW_X } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import DiffView from '@/components/fleet/DiffView.svelte'
  import { diffLines, type DiffLine } from '@/components/fleet/line-diff'

  interface Revision {
    id: string
    createdBy: string | null
    createdAt: string
    size: number
    /** Set for agent-version-backed kinds (soul/config/personality). */
    note?: string | null
    version?: number
  }

  /** THE WORKBENCH EDITOR, extracted from the modal that wrapped it: a near-
   *  fullscreen document surface — WYSIWYG editor filling the height, version
   *  history in a rail, streaming Muse drafting. Clicking a revision shows a
   *  DIFF against the editor's current content; loading one stages it in the
   *  editor (not saved until you Save), so a revert is itself reviewable and
   *  produces a revision.
   *
   *  The modal that used to wrap it (`InternalEditorModal`) is now a thin shell
   *  around this, and the record views embed this directly so the editor lives
   *  in the view instead of behind an Edit button. */
  let {
    subtitle,
    value,
    editable = true,
    saving,
    onSave,
    dirty = $bindable(false),
    showHistory = $bindable(false),
    history,
    footerExtra,
    mode = 'rich',
    muse,
    onClose,
    withActions = true,
    fill = false,
    nested = false,
    class: className,
  }: {
    subtitle?: string
    value: string
    editable?: boolean
    saving?: boolean
    onSave: (markdown: string) => Promise<void> | void
    /** Whether the editor holds unsaved work; bound so a surrounding record
     *  surface can run one Save for the whole record and one dirty flag. */
    dirty?: boolean
    /** The history rail's open state — owned by whoever gives this editor a
     *  title: a record surface pins the toggle on its title line
     *  (`bind:showHistory`); standalone, the editor menu carries it. Collapsed
     *  by default: the rail is a reference surface you summon while editing,
     *  not a column the document starts beside. */
    showHistory?: boolean
    /** Query params for /api/history (kind + owner/name or id). Omit to hide history. */
    history?: Record<string, string>
    /** Extra control on the left of the footer (e.g. a delete button). */
    footerExtra?: Snippet
    /** 'rich' (default) renders WYSIWYG markdown; 'plain' a mono text surface —
     *  for structured text like config YAML where prose rendering would lie. */
    mode?: 'rich' | 'plain'
    /** Enable AI drafting for this document: prompt → streamed proposal →
     *  review diff → accept, iterating with the conversation retained. */
    muse?: { kind: MuseKind; context?: string }
    /** Closes the surface — the footer's Close button and, in `nested` mode,
     *  this panel's own Esc. */
    onClose?: () => void
    /** Render the footer's Close/Save row. A record surface running its own
     *  Save for the whole record sets this false and drives `onSave` itself. */
    withActions?: boolean
    /** Fill the parent's height instead of the default 76vh fixed box. */
    fill?: boolean
    /** This panel sits inside another modal and Esc must close IT, not the
     *  modal. Requires a positioned ancestor — the takeover Modal's panel
     *  provides one. */
    nested?: boolean
    class?: string
  } = $props()

  let editorRef = $state<RichEditorHandle | null>(null)
  let seed = $state(0) // bump to remount the editor with new content
  let current = $state(value)
  let diffing = $state<{ rev: Revision; content: string; diff: DiffLine[] | null } | null>(null)

  // ── Muse: prompt → streamed proposal → review/accept, iteratively ───────
  let museOpen = $state(false)
  let instruction = $state('')
  let generating = $state(false)
  let proposal = $state<string | null>(null)
  let proposalDiff = $state<DiffLine[] | null | 'text'>('text')
  let chat = $state<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  let museError = $state<string | null>(null)
  let abortRef: AbortController | null = null
  let streamEl = $state<HTMLPreElement | null>(null)

  // Keep the streaming <pre> pinned to the bottom as pieces arrive.
  $effect(() => {
    void proposal
    if (generating && streamEl) streamEl.scrollTop = streamEl.scrollHeight
  })
  // Abort any in-flight stream on unmount.
  $effect(() => () => abortRef?.abort())

  const generate = async () => {
    const instr = instruction.trim()
    if (!instr || !muse) return
    generating = true
    museError = null
    diffing = null
    proposal = ''
    proposalDiff = 'text'
    const ac = new AbortController()
    abortRef = ac
    try {
      const full = await streamMuse(
        { kind: muse.kind, context: muse.context, current: getMarkdown(), instruction: instr, chat },
        (piece) => (proposal = (proposal ?? '') + piece),
        ac.signal,
      )
      chat = [...chat.slice(-10), { role: 'user', content: instr }, { role: 'assistant', content: full }]
      instruction = ''
    } catch (e) {
      if (!ac.signal.aborted) {
        museError = (e as Error).message
        proposal = null
      }
    } finally {
      generating = false
    }
  }

  const acceptProposal = () => {
    if (proposal === null) return
    current = proposal
    seed += 1
    dirty = true
    proposal = null
  }

  // The LIST read has no 404 (a doc with no snapshots is a 200 with `[]`), so
  // every non-2xx is a failure. Answering `[]` printed "No saved revisions yet"
  // over a broken history service — and that is the sentence someone believes
  // right before they overwrite a document they thought had no backups.
  const historyQuery = createQuery(() => ({
    queryKey: ['history', history],
    enabled: !!history,
    queryFn: (): Promise<Revision[]> => getList<Revision>(`/api/history?${new URLSearchParams(history).toString()}`, 'revisions'),
  }))

  const save = async () => {
    await onSave(getMarkdown())
    dirty = false
    void historyQuery.refetch()
  }

  const fetchRevision = async (id: string): Promise<string | null> => {
    if (!history) return null
    const qs = new URLSearchParams({ ...history, rev: id }).toString()
    const r = await fetch(`/api/history?${qs}`)
    if (!r.ok) return null
    return ((await r.json()) as { content: string }).content
  }

  /** Stage a revision's content in the editor (unsaved). */
  const loadRevision = async (id: string, prefetched?: string) => {
    const content = prefetched ?? (await fetchRevision(id))
    if (content === null) return
    diffing = null
    current = content
    seed += 1
    dirty = true
  }

  /** Show what changed between a revision and the editor's current content. */
  const openDiff = async (rev: Revision) => {
    const content = await fetchRevision(rev.id)
    if (content === null) return
    diffing = { rev, content, diff: diffLines(content, getMarkdown()) }
  }

  /** THE SURROUNDING RECORD SURFACE'S HANDLES. `getMarkdown` is what the
   *  record's Save writes; `setDoc` is how a whole-form Muse draft lands in
   *  the editor — accepted into the fields, never written, still reviewable. */
  export function getMarkdown(): string {
    return editorRef?.getMarkdown() ?? current
  }

  export function setDoc(markdown: string): void {
    current = markdown
    seed += 1
    dirty = true
  }

  /** Stage a document WITHOUT marking it dirty — Cancel restoring the saved
   *  text is not unsaved work. */
  export function restoreDoc(markdown: string): void {
    current = markdown
    seed += 1
    dirty = false
  }

  // Nested mode: Esc closes THIS panel only — a capture-phase listener runs
  // before (and suppresses) the parent modal's document-level Esc handler.
  $effect(() => {
    if (!nested || !onClose) return
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    document.addEventListener('keydown', h, true)
    return () => document.removeEventListener('keydown', h, true)
  })
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class={cn('flex flex-col gap-3', fill ? 'h-full min-h-0' : 'h-[76vh]', className)}
  onkeydown={(e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 's') {
      e.preventDefault()
      // A record surface embedding this runs its own ⌘S for the whole record:
      // stop the event here so one shortcut is not two saves.
      e.stopPropagation()
      if (editable && dirty && !saving) void save()
    }
  }}
>
  {#if subtitle}<p class="shrink-0 text-xs text-muted">{subtitle}</p>{/if}
  <div class="flex min-h-0 flex-1 gap-3">
    <div class="flex min-w-0 flex-1 flex-col">
      {#if proposal !== null}
        <!-- Tab-pane grammar: the proposal / diff views rise in on switch (no
             exit). The editor views below keep their hard cut — they carry
             unsaved input and are seed-keyed, so an entrance would replay on
             every Muse accept / revision load. -->
        <div in:fly={{ y: 6, duration: 200 }} class="flex min-h-0 flex-1 flex-col">
        <div class="mb-2 flex items-center gap-2 text-xs text-muted">
          <Sparkles size={13} class="shrink-0 text-accent" />
          {#if generating}
            <span>Drafting</span>
            <Button variant="ghost" size="sm" class="ml-auto shrink-0" onclick={() => abortRef?.abort()}>
              <Square size={12} fill="currentColor" /> Stop
            </Button>
          {:else}
            <span>Proposal: review, then accept or refine below.</span>
            <Button size="sm" class="ml-auto shrink-0" onclick={acceptProposal}>
              <Check size={13} /> Accept
            </Button>
            <Button
              variant="outline"
              size="sm"
              class="shrink-0"
              onclick={() => (proposalDiff = proposalDiff === 'text' ? diffLines(getMarkdown(), proposal ?? '') : 'text')}
            >
              {proposalDiff === 'text' ? 'View changes' : 'View text'}
            </Button>
            <Button variant="ghost" size="sm" class="shrink-0" onclick={() => (proposal = null)}>
              <X size={13} /> Discard
            </Button>
          {/if}
        </div>
        <div class="min-h-0 flex-1">
          {#if proposalDiff === 'text' || generating}
            <pre
              bind:this={streamEl}
              class="h-full overflow-y-auto whitespace-pre-wrap rounded-lg border border-[var(--theme-accent-border,var(--theme-accent))] p-3 font-mono text-xs leading-5 text-fg"
            >{proposal}{#if generating}<span class="gd-pulse text-accent">▍</span>{/if}</pre>
          {:else}
            <!-- This branch means proposalDiff !== 'text', so it's the computed lines (or null). -->
            <DiffView diff={proposalDiff} fallback={proposal ?? ''} />
          {/if}
        </div>
        </div>
      {:else if diffing}
        <div in:fly={{ y: 6, duration: 200 }} class="flex min-h-0 flex-1 flex-col">
        <div class="mb-2 flex items-center gap-2 text-xs text-muted">
          <span>
            Changes since {diffing.rev.version !== undefined ? `v${diffing.rev.version} · ` : ''}
            {relativeTime(diffing.rev.createdAt)}
            {diffing.rev.createdBy ? ` · ${diffing.rev.createdBy}` : ''}. Additions are what the current text
            gained, removals what it lost.
          </span>
          {#if editable}
            <Button variant="outline" size="sm" class="ml-auto shrink-0" onclick={() => void loadRevision(diffing!.rev.id, diffing!.content)}>
              <RotateCcw size={13} /> Load into editor
            </Button>
          {/if}
          <Button variant="ghost" size="sm" class={cn('shrink-0', !editable && 'ml-auto')} onclick={() => (diffing = null)}>
            <X size={13} /> Close diff
          </Button>
        </div>
        <div class="min-h-0 flex-1">
          <DiffView diff={diffing.diff} fallback={diffing.content} />
        </div>
        </div>
      {:else if mode === 'plain'}
        {#if editable}
          {#key seed}
            <textarea
              bind:value={current}
              oninput={() => (dirty = true)}
              spellcheck="false"
              class="min-h-0 w-full flex-1 resize-none rounded-md border border-line bg-[var(--theme-input)] p-3 font-mono text-xs leading-5 text-fg outline-none transition-colors focus:border-accent focus:ring-2 focus:ring-accent-soft"
            ></textarea>
          {/key}
        {:else}
          <pre class="min-h-0 flex-1 overflow-y-auto whitespace-pre-wrap rounded-lg border border-line bg-surface p-3 font-mono text-xs leading-5 text-fg">{current}</pre>
        {/if}
      {:else}
        <div class="min-h-0 flex-1 overflow-y-auto">
          {#key seed}
            <RichEditor
              bind:this={editorRef}
              value={current}
              {editable}
              onSave={() => (dirty = true)}
              placeholder={editable ? 'Write in plain language. Formatting is saved as markdown.' : undefined}
              class="h-full"
              fill
            />
          {/key}
        </div>
      {/if}
    </div>
    {#if showHistory && history}
      <!-- IN-FLOW rail: slide={GROW_X} on both legs so the editor glides as
           the rail grows/shrinks instead of snapping (ANIMATIONS.md).
           |global: this editor often mounts inside an {#if} (a modal opened
           on click, a keyed record) where a local intro would be suppressed —
           |global keeps the rail's first entrance animated there too. Inner
           wrapper pinned to the resting width so rows clip, not rewrap. -->
      <div transition:slide|global={GROW_X} class="shrink-0 overflow-y-auto rounded-lg border border-line">
      <div class="w-64 p-1">
        <div class="px-2 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">History</div>
        <QueryState query={historyQuery} errorTitle="Could not load history" errorVariant="inline">
          {#snippet skeleton()}<SkeletonRows rows={4} class="px-2 py-2" />{/snippet}
          {#snippet empty()}<div class="px-2 py-2 text-xs text-muted">No saved revisions yet.</div>{/snippet}
          {#snippet children(revisions)}
            <div use:listStagger>
            {#each revisions as rev, i (rev.id)}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <div
                class={cn(
                  'group flex items-center gap-2 rounded-md px-2 py-1.5 text-xs',
                  diffing?.rev.id === rev.id && 'bg-raised',
                  i !== 0 && 'cursor-pointer transition-colors dither-fill',
                )}
                onclick={i === 0 ? undefined : () => void openDiff(rev)}
                title={i === 0 ? undefined : 'Show changes since this revision'}
              >
                <div class="min-w-0 flex-1">
                  <div class="truncate text-fg">
                    {#if rev.version !== undefined}<span class="mr-1.5 font-mono text-accent">v{rev.version}</span>{/if}
                    {i === 0 ? 'Current' : relativeTime(rev.createdAt)}
                  </div>
                  <div class="truncate font-mono text-[11px] text-muted">
                    {rev.createdBy ?? 'unknown'} · {rev.size.toLocaleString()} chars
                  </div>
                  {#if rev.note}<div class="truncate text-[11px] italic text-muted">{rev.note}</div>{/if}
                </div>
                {#if editable && i !== 0}
                  <button
                    type="button"
                    title="Load this revision into the editor"
                    onclick={(e) => {
                      e.stopPropagation()
                      void loadRevision(rev.id)
                    }}
                    class="shrink-0 text-muted opacity-0 transition-all group-hover:opacity-100 hover:text-accent"
                  >
                    <RotateCcw size={13} />
                  </button>
                {/if}
              </div>
            {/each}
            </div>
          {/snippet}
        </QueryState>
      </div>
      </div>
    {/if}
  </div>
  {#if museOpen && muse && editable}
      <div transition:slide={{ duration: 150 }} class="flex shrink-0 items-end gap-2.5">
      <Sparkles size={14} class="mb-3 shrink-0 text-accent" />
      <Textarea
        autoGrow
        rows={1}
        bind:value={instruction}
        onkeydown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault()
            if (!generating && instruction.trim()) void generate()
          }
        }}
        placeholder={proposal !== null
          ? 'Refine the proposal, e.g. “shorter, and add a step for weekends”'
          : 'Describe what you want. It drafts from the current version'}
        class="max-h-40 text-sm"
        autofocus
      />
      <Button class="shrink-0" onclick={() => void generate()} disabled={generating || !instruction.trim()}>
        {generating ? 'Drafting' : proposal !== null ? 'Refine' : 'Draft'}
      </Button>
    </div>
  {/if}
  {#if museError}<p transition:slide={{ duration: 150 }} class="shrink-0 text-xs text-danger">{museError}</p>{/if}

  {#if (muse && editable) || footerExtra || history || withActions}
    <!-- THE EDITOR MENU — the workbench's own controls, tied to the editor it
         belongs to: it carries NO top border under the document, so the only
         line at the bottom of a surface belongs to the pinned menu below. In a
         standalone modal (`withActions`) this row IS the pinned menu, and it
         takes the border there. -->
    <div class={cn('flex shrink-0 items-center gap-2 pt-3', withActions && 'border-t border-line')}>
      {#if muse && editable}
        <Button
          variant={museOpen ? 'outline' : 'ghost'}
          size="sm"
          onclick={() => (museOpen = !museOpen)}
          title="Draft with AI. Uses your preferred model (Settings)"
        >
          <Sparkles size={14} class="mr-1.5" /> Muse
        </Button>
      {/if}
      {@render footerExtra?.()}
      {#if history && withActions}
        <!-- Standalone: no title line above to carry it, so the toggle lives
             here, right-aligned. Embedded surfaces own it themselves. -->
        <Button
          variant={showHistory ? 'outline' : 'ghost'}
          size="sm"
          class="ml-auto shrink-0"
          onclick={() => (showHistory = !showHistory)}
          title="Version history"
          aria-pressed={showHistory}
        >
          <History size={14} />
        </Button>
      {/if}
      {#if withActions}
        <span class={cn('font-mono text-[10px] uppercase tracking-[0.05em] text-muted', !history && 'ml-auto')}>
          {editable && dirty ? 'Unsaved changes · ⌘S to save' : ''}
        </span>
        <Button variant="ghost" size="sm" onclick={onClose}>
          Close
        </Button>
        {#if editable}
          <Button size="sm" onclick={() => void save()} disabled={saving || !dirty}>
            {saving ? 'Saving' : 'Save'}
          </Button>
        {/if}
      {/if}
    </div>
  {/if}
</div>
