<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronRight, Globe, History, Link2, ListTree, Maximize2, MessageSquareText, Minimize2, Pencil, Sparkles, Star, Trash2 } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { copyAppLink, useContextMenu } from '@/components/ui/context-menu.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import { listQuery } from '@/components/ui/query-state'
  import PermissionsModal from '@/components/kb/PermissionsModal.svelte'
  import BrainRoutingSelect from '@/components/kb/BrainRoutingSelect.svelte'
  import { cn } from '@/lib/cn'
  import { fade, fly, slide, GROW_X } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { useSession } from '@/lib/session'
  import { deleteDoc, saveDoc, useBacklinks, useDoc, type KbDocMeta } from '@/lib/kb'
  import KbArtifactAttachments from './KbArtifactAttachments.svelte'
  import KbCommentsPanel from './KbCommentsPanel.svelte'
  import KbDocPageSkeleton from './KbDocPageSkeleton.svelte'
  import KbHistoryRail from './KbHistoryRail.svelte'
  import KbMuseBar from './KbMuseBar.svelte'
  import KbSettingsMenu from './KbSettingsMenu.svelte'
  import KbTocPanel from './KbTocPanel.svelte'
  import KbVisibilityIcon from './KbVisibilityIcon.svelte'
  import { docSearch, editorShell, parseHeadings, useDocComments, useDocLive, useIsOwner } from './knowledge.svelte'

  let {
    docId,
    docs,
    onDeleted,
    onSelect,
    folderName,
  }: {
    docId: string
    docs: KbDocMeta[]
    onDeleted: () => void
    onSelect: (id: string) => void
    folderName?: string
  } = $props()

  const qc = useQueryClient()
  const docQuery = useDoc(() => docId)
  const doc = $derived(docQuery.data)
  const session = useSession()
  const me = $derived(session.data)
  // "Nothing links here" is a claim about the whole knowledge base; a failed
  // read must not make it.
  const backlinksList = listQuery(useBacklinks(() => docId), { title: 'Could not load backlinks', variant: 'inline' })
  const backlinks = $derived(backlinksList.rows)
  const backlinksLoading = $derived(backlinksList.pending)
  let editorRef = $state<RichEditor | null>(null)
  let bodyRef = $state<HTMLDivElement | null>(null)
  let title = $state('')
  let dirty = $state(false)
  let showHistory = $state(false)
  let showToc = $state(false)
  let emojiOpen = $state(false)
  let fullscreen = $state(false)
  let shareOpen = $state(false)
  let seed = $state(0) // bump to remount the editor (e.g. after restore)
  const ownFallback = useIsOwner(() => doc)
  // Sharing governance: server-computed (covers agent-created docs any
  // agent-user may govern); fall back to plain ownership for older payloads.
  const isOwner = $derived(doc?.governs ?? ownFallback.current)
  // Authored docs open in read mode (like tickets); empty ones open in edit.
  let mode = $state<'read' | 'edit'>('read')
  let showComments = $state(false)
  let focusThread = $state<string | null>(null)
  let museSel = $state<{ text: string; source: 'read' | 'editor' } | null>(null)
  let okfOpen = $state(false)
  const docMenu = useContextMenu()
  let readRef = $state<HTMLDivElement | null>(null)
  let pendingQuote = $state<string | null>(null)
  let selPop = $state<{ x: number; y: number; quote: string } | null>(null)
  // Presence drives the multiplayer avatars and the read-mode auto-refresh.
  // Defaulted it says "you are alone in here" during an outage, which is when
  // two people are most likely to overwrite each other.
  const presenceList = listQuery(useDocLive(docId, () => mode), { title: 'Could not see who else is here', variant: 'inline' })
  const presence = $derived(presenceList.rows)
  const commentsQuery = useDocComments(docId)
  const comments = $derived(commentsQuery.data ?? [])
  const openThreads = $derived(comments.filter((c) => !c.parentId && !c.resolved).length)
  const otherEditors = $derived(presence.filter((p) => p.userId !== me?.id && p.mode === 'edit'))
  let initMode = false
  $effect(() => {
    if (doc) title = doc.title
    if (doc && !initMode) {
      initMode = true
      mode = doc.body.trim() ? 'read' : 'edit'
    }
  })

  // Multiplayer read freshness: while others are here, the rendered doc
  // follows their saves (edit mode never yanks your buffer).
  $effect(() => {
    if (mode !== 'read' || presence.length <= 1) return
    const t = setInterval(() => void qc.invalidateQueries({ queryKey: ['kb-doc', docId] }), 10_000)
    return () => clearInterval(t)
  })

  // Quote-anchored highlights: after render, find each OPEN thread's quote in
  // the read surface and wrap it in a clickable mark. Idempotent — old marks
  // unwrap first; single-text-node matches only (quotes are plain sentences).
  $effect(() => {
    void doc?.body
    const host = readRef
    if (!host || mode !== 'read') return
    for (const old of Array.from(host.querySelectorAll('[data-kb-mark]'))) {
      const parent = old.parentNode
      if (!parent) continue
      while (old.firstChild) parent.insertBefore(old.firstChild, old)
      parent.removeChild(old)
      parent.normalize()
    }
    const targets = comments.filter((c) => !c.parentId && !c.resolved && c.quote?.trim())
    for (const c of targets) {
      const quote = c.quote!.replace(/\s+/g, ' ').trim()
      const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT)
      let node: Text | null
      while ((node = walker.nextNode() as Text | null)) {
        if (node.parentElement?.closest('[data-kb-mark]')) continue
        const text = node.textContent ?? ''
        const idx = text.replace(/\s+/g, ' ').indexOf(quote)
        if (idx === -1) continue
        // Map the normalized index back — safe when the node has no runs of
        // whitespace; bail to a plain indexOf otherwise.
        const rawIdx = text.indexOf(c.quote!.trim()) !== -1 ? text.indexOf(c.quote!.trim()) : idx
        try {
          const range = document.createRange()
          range.setStart(node, rawIdx)
          range.setEnd(node, Math.min(rawIdx + c.quote!.trim().length, text.length))
          const mark = document.createElement('span')
          mark.className = 'kb-comment-mark'
          mark.dataset.kbMark = c.id
          mark.title = `${c.author}: ${c.content.slice(0, 80)}`
          range.surroundContents(mark)
        } catch {
          /* range crossed an element boundary — skip this quote */
        }
        break
      }
    }
  })

  const headings = $derived(parseHeadings(doc?.body ?? ''))

  // Breadcrumb: walk parentId up from this doc.
  const trail = $derived.by(() => {
    const chain: KbDocMeta[] = []
    let cur = docs.find((d) => d.id === docId)
    const seen = new Set<string>()
    while (cur && !seen.has(cur.id)) {
      seen.add(cur.id)
      chain.unshift(cur)
      cur = cur.parentId ? docs.find((d) => d.id === cur!.parentId) : undefined
    }
    return chain
  })

  const save = async (patch: Parameters<typeof saveDoc>[1]) => {
    await saveDoc(docId, patch)
    await qc.invalidateQueries({ queryKey: ['kb-doc', docId] })
    await qc.invalidateQueries({ queryKey: ['kb-docs', doc?.spaceId] })
    dirty = false
  }
  const saveBody = () => save({ title, body: editorRef?.getMarkdown() ?? doc?.body ?? '' })

  // Scroll the rendered editor to the Nth heading (headings render in order).
  const scrollToHeading = (index: number) => {
    const nodes = bodyRef?.querySelectorAll('.tiptap h1, .tiptap h2, .tiptap h3')
    nodes?.[index]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
</script>

<!-- Esc leaves fullscreen (unless a menu/popup is open and swallows it first). -->
<svelte:window
  onkeydown={(e) => {
    if (fullscreen && e.key === 'Escape') fullscreen = false
  }}
/>

<!-- Same three answers, same collapse: twelve skeleton bars that never resolve,
     for a 500 and for a document that genuinely no longer exists. -->
{#if docQuery.isError && doc === undefined}
  <div in:fade={{ duration: 150 }}>
    <QueryError
      error={docQuery.error}
      title="Could not load this document"
      onRetry={() => void docQuery.refetch()}
    />
  </div>
{:else if doc === null}
  <div in:fade={{ duration: 150 }}>
    <EmptyState icon="⧉" title="Document not found" hint="It may have been deleted, or you don’t have access." />
  </div>
{:else if !doc}
  <KbDocPageSkeleton breadcrumb bars={12} />
{:else}
  <div class={editorShell(fullscreen)}>
    <!-- Breadcrumb -->
    <div class="flex items-center gap-1 border-b border-line-subtle px-6 pt-2 text-[11px] text-muted">
      {#each trail as d, i (d.id)}
        <span class="flex items-center gap-1">
          {#if i > 0}<ChevronRight size={11} class="opacity-50" />{/if}
          <button
            type="button"
            onclick={() => d.id !== docId && onSelect(d.id)}
            class={cn('max-w-[12rem] truncate font-sans', d.id === docId ? 'text-fg' : 'hover:text-fg')}
          >
            {d.icon ? `${d.icon} ` : ''}{d.title}
          </button>
        </span>
      {/each}
    </div>

    <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
      <div class="relative shrink-0">
        <button
          type="button"
          onclick={() => (emojiOpen = !emojiOpen)}
          class="rounded-md px-1 text-xl leading-none transition-colors dither-fill"
          title="Set icon"
        >
          {doc.icon ?? '📄'}
        </button>
        {#if emojiOpen}
          <EmojiPicker
            onPick={(e) => {
              void save({ icon: e })
              emojiOpen = false
            }}
            onClear={() => {
              void save({ icon: null })
              emojiOpen = false
            }}
            onClose={() => (emojiOpen = false)}
          />
        {/if}
      </div>
      {#if mode === 'edit'}
        <Input
          value={title}
          oninput={(e) => {
            title = e.currentTarget.value
            dirty = true
          }}
          onblur={() => dirty && void saveBody()}
          onkeydown={inlineEditKeys(() => doc && (title = doc.title))}
          class="min-w-0 flex-1 border-0 bg-transparent text-lg font-semibold focus:border-0"
          placeholder="Untitled"
        />
      {:else}
        <div class="min-w-0 flex-1">
          <h1 class="truncate font-sans text-lg font-semibold text-fg">{doc.title}</h1>
          <!-- Timestamp meta rides in the mono chrome voice (spec §2). -->
          <div class="truncate font-mono text-[10px] tracking-[0.05em] text-muted">
            edited {relativeTime(doc.updatedAt)}{doc.updatedBy ? ` by ${doc.updatedBy}` : ''}
          </div>
        </div>
      {/if}
      {#if doc.okf}
        <Button variant="ghost" size="xs" class="shrink-0 rounded border border-accent/40 bg-accent/10 px-1.5 text-accent hover:bg-accent/20" onclick={() => (okfOpen = true)}
          
          title="This promoted document carries an agent-facing OKF summary, maintained by the Librarian. Click to view.">
          OKF
        </Button>
      {/if}
      {#if presenceList.notice}<QueryError {...presenceList.notice} />{/if}
      <!-- Who's here: green ring = editing right now. -->
      {#if presence.length > 1}
        <div class="flex shrink-0 -space-x-1.5">
          {#each presence.slice(0, 5) as p (p.userId)}
            <span class="relative" title={`${p.name}${p.mode === 'edit' ? ' (editing)' : ''}`}>
              <Avatar
                name={p.name}
                class={cn('h-6 w-6 text-[10px] ring-2 ring-surface', p.mode === 'edit' && 'ring-success')}
              />
              {#if p.mode === 'edit'}
                <Pencil size={8} class="absolute -bottom-0.5 -right-0.5 rounded-full bg-surface text-success" />
              {/if}
            </span>
          {/each}
        </div>
      {/if}
      {#if mode === 'edit' && otherEditors.length > 0}
        <span
          class="shrink-0 rounded-md bg-warning/15 px-2 py-0.5 text-[11px] text-warning"
          title="Someone else is editing too. Last save wins, so coordinate or take turns"
        >
          also editing: {otherEditors.map((p) => p.name).join(', ')}
        </span>
      {/if}
      <Button
        variant={showComments ? 'outline' : 'ghost'}
        size="sm"
        class="shrink-0"
        title="Comments"
        onclick={() => (showComments = !showComments)}
      >
        <MessageSquareText size={14} />
        {#if openThreads > 0}<span class="ml-1 text-[11px] text-accent">{openThreads}</span>{/if}
      </Button>
      <!-- Read / Edit toggle — authored docs open in read mode. -->
      <Segmented
        size="xs"
        options={[{ id: 'read', label: 'Read' }, { id: 'edit', label: 'Edit' }] as const}
        value={mode}
        onChange={(m) => {
          if (m === 'read' && mode === 'edit') void saveBody() // capture edits on exit
          mode = m
        }}
      />
      <!-- Sharing + promotion: one quiet cluster. The Official pill is a
           STATE — promoting confirms once, demoting confirms twice (it pulls
           the doc out of what grounds every agent). -->
      <Button variant="ghost" size="sm" class="shrink-0" title="Share & permissions" onclick={() => (shareOpen = true)}>
        <KbVisibilityIcon v={doc.visibility} /> <span class="ml-1.5 capitalize">{doc.visibility}</span>
      </Button>
      {#if doc.official}
        <Button variant="ghost" size="xs" class="shrink-0 gap-1 border border-warning/50 bg-warning/10 px-2.5 py-1 text-warning hover:bg-warning/20" title="Official: grounds every agent via the org brain. Click to demote (double confirm)."
          onclick={async () => {
            if (!(await confirm({ title: 'Demote from official?', message: `“${doc.title}” currently grounds every agent through the org brain. Demoting removes it from retrieval.`, confirmLabel: 'Continue' }))) return
            if (!(await confirm({ title: 'Really demote?', message: 'Agents stop grounding on this document immediately. This is the final confirmation.', confirmLabel: 'Demote', danger: true }))) return
            void save({ official: false })
          }}>
          <Star size={12} fill="currentColor" /> Official
        </Button>
      {:else}
        <Button
          variant="ghost"
          size="sm"
          class="shrink-0"
          title="Promote to official: indexed into the org brain your agents ground on"
          onclick={async () => {
            if (await confirm({ title: 'Promote to official?', message: `“${doc.title}” will be indexed into the org brain and ground every agent's answers.`, confirmLabel: 'Promote' })) {
              void save({ official: true })
            }
          }}
        >
          <Star size={13} class="mr-1" /> Promote
        </Button>
      {/if}
      <BrainRoutingSelect value={doc.ragRouting} canEdit={isOwner} onChange={(ragRouting) => void save({ ragRouting })} />
      <span class="mx-0.5 h-4 shrink-0 border-l border-line-subtle"></span>
      <Button variant={showToc ? 'outline' : 'ghost'} size="sm" class="shrink-0" title="Table of contents" onclick={() => (showToc = !showToc)}>
        <ListTree size={14} />
      </Button>
      <Button variant="ghost" size="sm" class="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onclick={() => (fullscreen = !fullscreen)}>
        {#if fullscreen}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
      </Button>
      <KbSettingsMenu
        items={[
          { label: showHistory ? 'Hide history' : 'Version history', icon: History, onClick: () => (showHistory = !showHistory) },
          {
            label: 'Delete document',
            icon: Trash2,
            danger: true,
            onClick: async () => {
              if (!(await confirm({ title: 'Delete document', message: `Delete "${doc.title}"?`, confirmLabel: 'Delete', danger: true }))) return
              await deleteDoc(docId)
              await qc.invalidateQueries({ queryKey: ['kb-docs', doc.spaceId] })
              onDeleted()
            },
          },
        ]}
      />
    </div>

    {#if doc.visibility === 'public' && doc.publicSlug}
      <div class="flex items-center gap-2 border-b border-line-subtle bg-panel px-6 py-1.5 font-mono text-[11px] text-muted">
        <Globe size={12} /> Public link:
        <code class="text-fg">{typeof window !== 'undefined' ? `${window.location.origin}/kb/${doc.publicSlug}` : `/kb/${doc.publicSlug}`}</code>
      </div>
    {/if}

    <div bind:this={bodyRef} class="flex min-h-0 flex-1">
      {#if mode === 'edit'}
        <!-- Flush page surface: the editor fills the panel, text wraps to a
             comfortable centered measure, and it autosaves as you type. -->
        <div
          class="flex min-w-0 flex-1 flex-col"
          role="presentation"
          oncontextmenu={(e) => {
            // Our menu replaces the native one inside the editor: formatting
            // + table structure on the selection, Muse, copy. (Global
            // suppression exempts contenteditable, so this is deliberate.)
            const sel = editorRef?.getSelectionText() ?? ''
            const inTable = editorRef?.isInTable() ?? false
            docMenu.openMenu(e, [
              { label: 'Bold', disabled: !sel, onSelect: () => editorRef?.toggleMark('bold') },
              { label: 'Italic', disabled: !sel, onSelect: () => editorRef?.toggleMark('italic') },
              { label: 'Strikethrough', disabled: !sel, onSelect: () => editorRef?.toggleMark('strike') },
              { label: 'Inline code', disabled: !sel, onSelect: () => editorRef?.toggleMark('code') },
              'sep',
              ...(inTable
                ? ([
                    { label: 'Add row below', onSelect: () => editorRef?.tableCommand('addRowAfter') },
                    { label: 'Add row above', onSelect: () => editorRef?.tableCommand('addRowBefore') },
                    { label: 'Add column right', onSelect: () => editorRef?.tableCommand('addColumnAfter') },
                    { label: 'Add column left', onSelect: () => editorRef?.tableCommand('addColumnBefore') },
                    { label: 'Delete row', onSelect: () => editorRef?.tableCommand('deleteRow') },
                    { label: 'Delete column', onSelect: () => editorRef?.tableCommand('deleteColumn') },
                    { label: 'Delete table', danger: true, onSelect: () => editorRef?.tableCommand('deleteTable') },
                    'sep',
                  ] as const)
                : ([{ label: 'Insert table', onSelect: () => editorRef?.tableCommand('insertTable') }, 'sep'] as const)),
              { label: 'Ask Muse about selection', disabled: !sel, onSelect: () => (museSel = { text: sel, source: 'editor' }) },
              { label: 'Copy selection', disabled: !sel, onSelect: () => void navigator.clipboard.writeText(sel) },
              { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?d=${docId}`) },
            ])
          }}
        >
          {#key `${docId}-${seed}`}
            <RichEditor
              bind:this={editorRef}
              value={doc.body}
              {docSearch}
              slash
              prose
              autosave
              onSave={() => void saveBody()}
              placeholder={doc.kind === 'agent' ? 'OKF-structured knowledge for agents' : 'Write'}
              fill
              class="min-w-0 flex-1"
            />
          {/key}
          <KbMuseBar
            context={`Knowledge document “${title || doc.title}”${folderName ? ` in the “${folderName}” space` : ''}.`}
            currentText={() => editorRef?.getMarkdown() ?? doc.body}
            selection={museSel?.text ?? null}
            onClearSelection={() => (museSel = null)}
            surgical={!!museSel}
            onAccept={async (md) => {
              await save({ title, body: md })
              seed += 1
              museSel = null
            }}
            onAcceptSelection={async (replacement) => {
              if (!museSel) return
              if (museSel.source === 'editor') {
                editorRef?.replaceSelection(replacement)
                await saveBody()
              } else {
                const body = editorRef?.getMarkdown() ?? doc.body
                await save({ title, body: body.replace(museSel.text, replacement) })
                seed += 1
              }
              museSel = null
            }}
          />
        </div>
      {:else}
        <!-- Read mode: rendered markdown with the identical measure/typography as
             the editor (both use .re-prose), so switching modes doesn't reflow.
             Tab-pane entrance on the READ pane only — the edit pane holds live
             editor state (seed-keyed for Muse/revisions), so it keeps its hard
             cut rather than replaying an entrance. -->
        <div in:fly={{ y: 6, duration: 200 }} class="flex min-w-0 flex-1 flex-col">
          <div
            bind:this={readRef}
            class="re-prose relative min-w-0 flex-1 overflow-y-auto"
            role="presentation"
            oncontextmenu={(e) => {
              const sel = window.getSelection()?.toString().trim() ?? ''
              docMenu.openMenu(e, [
                { label: 'Copy text', disabled: !sel && !doc.body, onSelect: () => void navigator.clipboard.writeText(sel || doc.body) },
                { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?d=${docId}`) },
                ...(sel
                  ? [
                      {
                        label: 'Comment on selection',
                        onSelect: () => {
                          pendingQuote = sel.slice(0, 500)
                          showComments = true
                        },
                      },
                      { label: 'Ask Muse about selection', onSelect: () => (museSel = { text: sel, source: 'read' as const }) },
                    ]
                  : []),
                { label: showComments ? 'Hide comments' : 'Comments', onSelect: () => (showComments = !showComments) },
              ])
            }}
            onclick={(e) => {
              const mark = (e.target as HTMLElement).closest?.('[data-kb-mark]') as HTMLElement | null
              if (mark?.dataset.kbMark) {
                showComments = true
                focusThread = mark.dataset.kbMark
              }
            }}
            onmouseup={() => {
              const sel = window.getSelection()
              const text = sel?.toString().trim() ?? ''
              if (!text || text.length > 500 || !sel || sel.rangeCount === 0) {
                selPop = null
                return
              }
              const rect = sel.getRangeAt(0).getBoundingClientRect()
              const host = bodyRef?.getBoundingClientRect()
              if (!host) return
              selPop = { x: rect.left - host.left + rect.width / 2, y: rect.top - host.top - 8, quote: text }
            }}
          >
            {#if selPop}
              <div
                style:left="{selPop.x}px"
                style:top="{Math.max(selPop.y, 4)}px"
                class="absolute z-20 flex -translate-x-1/2 -translate-y-full overflow-hidden rounded-md border border-line bg-panel text-xs text-fg shadow-[var(--theme-shadow-2)]"
              >
                <button
                  type="button"
                  class="px-2 py-1 transition-colors dither-fill"
                  onmousedown={(e) => {
                    e.preventDefault()
                    pendingQuote = selPop!.quote
                    showComments = true
                    selPop = null
                    window.getSelection()?.removeAllRanges()
                  }}
                >
                  <MessageSquareText size={12} class="mr-1 inline" /> Comment
                </button>
                <span class="my-1 border-l border-line-subtle"></span>
                <button
                  type="button"
                  class="px-2 py-1 text-accent transition-colors dither-fill"
                  onmousedown={(e) => {
                    e.preventDefault()
                    museSel = { text: selPop!.quote, source: 'read' }
                    selPop = null
                    window.getSelection()?.removeAllRanges()
                  }}
                >
                  <Sparkles size={12} class="mr-1 inline" /> Muse
                </button>
              </div>
            {/if}
            {#if doc.body.trim()}
              <Markdown class="tiptap" children={doc.body} />
            {:else}
              <div class="mx-auto max-w-[46rem] px-6 py-8">
                <button type="button" onclick={() => (mode = 'edit')} class="text-sm text-muted hover:text-fg">
                  This document is empty. Click to start writing.
                </button>
              </div>
            {/if}

            <!-- Backlinks — docs that reference this one. -->
            {#if backlinksLoading}
              <div aria-hidden="true" class="mx-auto max-w-[46rem] px-6 pb-10">
                <div class="border-t border-line-subtle pt-4">
                  <Skeleton class="mb-3 h-2.5 w-24 rounded-full" />
                  <SkeletonRows rows={2} />
                </div>
              </div>
            {/if}
            {#if backlinksList.notice}
              <div class="mx-auto max-w-[46rem] px-6 pb-4"><QueryError {...backlinksList.notice} /></div>
            {/if}
            {#if !backlinksLoading && backlinks.length > 0}
              <div class="mx-auto max-w-[46rem] px-6 pb-10">
                <div class="border-t border-line-subtle pt-4">
                  <div class="mb-2 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                    <Link2 size={12} /> Linked from
                  </div>
                  <div class="flex flex-wrap gap-2">
                    {#each backlinks as b (b.id)}
                      <button
                        type="button"
                        onclick={() => onSelect(b.id)}
                        class="flex items-center gap-1.5 rounded-md border border-line px-2 py-1 text-xs text-muted transition-colors dither-fill hover:text-fg"
                      >
                        <span>{b.icon ?? '📄'}</span>
                        <span class="max-w-[16rem] truncate">{b.title}</span>
                      </button>
                    {/each}
                  </div>
                </div>
              </div>
            {/if}

            <KbArtifactAttachments {docId} />
          </div>
          <KbMuseBar
            context={`Knowledge document “${title || doc.title}”${folderName ? ` in the “${folderName}” space` : ''}.`}
            currentText={() => doc.body}
            selection={museSel?.text ?? null}
            onClearSelection={() => (museSel = null)}
            surgical={!!museSel && doc.body.includes(museSel.text)}
            onAccept={async (md) => {
              await save({ title, body: md })
              museSel = null
            }}
            onAcceptSelection={async (replacement) => {
              if (!museSel) return
              await save({ title, body: doc.body.replace(museSel.text, replacement) })
              museSel = null
            }}
          />
        </div>
      {/if}

      {#if showComments}
        <KbCommentsPanel
          {docId}
          {comments}
          loadFailed={commentsQuery.isError && commentsQuery.data === undefined}
          loadError={commentsQuery.error}
          onRetryLoad={() => void commentsQuery.refetch()}
          meId={me?.id ?? null}
          docOwnerId={doc.ownerUserId}
          {pendingQuote}
          onQuoteConsumed={() => (pendingQuote = null)}
          focusId={focusThread}
          onFocusConsumed={() => (focusThread = null)}
          onClose={() => (showComments = false)}
        />
      {/if}
      {#if showToc}
        <KbTocPanel
          {headings}
          onJump={scrollToHeading}
          onClose={() => (showToc = false)}
          emptyText="No headings yet. Add one (H1–H3) and it shows up here."
        />
      {/if}
      {#if showHistory}
        <!-- IN-FLOW rail: slide={GROW_X} on both legs (ANIMATIONS.md); inner
             wrapper pinned to the resting width so rows clip, not rewrap. -->
        <div transition:slide={GROW_X} class="shrink-0 overflow-y-auto border-l border-line-subtle">
        <div class="w-64 p-3">
          <KbHistoryRail
            id={docId}
            onRestore={async (content) => {
              // Snapshots are stored as `# Title\n\n<body>`; split them back out.
              const m = /^#\s+(.*)\n+([\s\S]*)$/.exec(content)
              const t = m ? m[1]!.trim() : title
              const b = m ? m[2]! : content
              title = t
              await save({ title: t, body: b })
              seed += 1
            }}
          />
        </div>
        </div>
      {/if}
    </div>

    {#if okfOpen && doc.okf}
      <Modal open onClose={() => (okfOpen = false)} title="OKF: agent-facing summary">
        <div class="space-y-3">
          <p class="font-sans text-xs text-muted">
            What agents read instead of the full document: an Open Knowledge Format concept the Librarian maintains from the
            promoted content. It refreshes automatically when this document changes.
          </p>
          <!-- Code-family content sits in a ground-inset well (spec §1). -->
          <pre class="max-h-96 overflow-y-auto whitespace-pre-wrap rounded-md border border-line bg-surface p-3 font-mono text-xs leading-relaxed text-fg">{doc.okf}</pre>
          {#if isOwner}
            <div class="flex justify-end border-t border-line-subtle pt-3">
              <Button size="sm" variant="outline" onclick={() => void save({ regenerateOkf: true } as never)}>
                Regenerate now
              </Button>
            </div>
          {/if}
        </div>
      </Modal>
    {/if}
    <ContextMenu menu={docMenu} />
    <PermissionsModal
      open={shareOpen}
      onClose={() => (shareOpen = false)}
      kind="docs"
      id={docId}
      label={doc.title}
      visibility={doc.visibility}
      editPolicy={doc.editPolicy}
      publicSlug={doc.publicSlug}
      canManage={isOwner}
      inheritable
      inherited={doc.permsInherited}
      {folderName}
      onSave={(patch) => save(patch)}
    />
  </div>
{/if}
