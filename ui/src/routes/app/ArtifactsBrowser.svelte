<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { untrack } from 'svelte'
  import { ArrowDown, ArrowUp, Share2, Upload } from '@lucide/svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { copyAppLink, useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { confirm, prompt } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { fade, listStagger } from '@/lib/motion'
  import { deleteArtifact, deleteFolder, duplicateArtifact, duplicateFolder, updateFolder, type Artifact } from '@/lib/artifacts'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import ArtifactsRow from './ArtifactsRow.svelte'
  import ArtifactsTile from './ArtifactsTile.svelte'
  import MoveDialog from './MoveDialog.svelte'
  import {
    clearClipboard, clipboard, copyClipboard, cutClipboard, isCut, pruneClipboard,
  } from './files-clipboard.svelte'
  import { DRAG_MIME, ROW_GRID, type Drag, type Row, type SortDir, type SortKey } from './artifacts'

  // The browser: the room you're standing in. Everything a file manager is
  // expected to do lives here — sortable columns, multi-select, drag to move,
  // drop to upload, right-click for the rest — so the rail can be a list of
  // PLACES instead of a tree the user has to hold in their head.
  let {
    rows,
    loading,
    failure,
    view,
    sortKey,
    sortDir,
    onSort,
    activeId,
    canOrganize,
    folderId,
    onPaste,
    emptyTitle,
    emptyHint,
    onOpenFolder,
    onOpenArtifact,
    onAscend,
    onMove,
    onUpload,
    onRefresh,
    onDeleted,
    onProperties,
    onShare,
  }: {
    rows: Row[]
    loading: boolean
    failure: { title: string; error: unknown; retry: () => void } | null
    view: 'list' | 'grid'
    sortKey: SortKey
    sortDir: SortDir
    onSort: (k: SortKey) => void
    activeId: string | null
    /** Flat places (Shared, Official, Recent) are views, not locations: moving
     *  and folder-making are meaningless there, so they're off. */
    canOrganize: boolean
    /** Where the browser stands — the MoveDialog opens here, ⌘V pastes here. */
    folderId: string | null
    /** Paste the clipboard: undefined = the current folder, an id = into it. */
    onPaste: (into?: string) => Promise<unknown>
    emptyTitle: string
    emptyHint?: string
    onOpenFolder: (id: string) => void
    onOpenArtifact: (id: string) => void
    /** ← (list view): climb one level — the parent folder, or nothing at the
     *  place root. The page owns the folder trail, so it owns the climb. */
    onAscend: () => void
    onMove: (drag: NonNullable<Drag>, folderId: string | null) => Promise<unknown>
    onUpload: (files: File[], intoFolderId?: string) => Promise<unknown>
    onRefresh: () => Promise<unknown>
    onDeleted: (id: string) => void
    /** The page owns the dialog: it has the folder tree, the directory, and the
     *  place name the properties panel needs to describe a location. */
    onProperties: (row: Row) => void
    onShare: (row: Row) => void
  } = $props()

  const menu = useContextMenu()

  // Selection is keyed by type+id: a folder and an artifact can't collide, and
  // the key survives a re-sort (index-based selection would not).
  const keyOf = (r: Row) => `${r.type}:${r.id}`
  let selected = $state<Set<string>>(new Set())
  let anchor = $state<number | null>(null)
  let drag = $state<Drag>(null)
  let overFolder = $state<string | null>(null)
  let fileOver = $state(false)
  let uploading = $state(false)
  let moveDialog = $state(false)

  // ── Focus: the roving tabindex ─────────────────────────────────────────────
  // Exactly one row carries tabindex=0 (the focused one); arrows move it. A
  // click or Tab focuses a row natively — onfocusin mirrors that into focusKey
  // so the keyboard model and the DOM never disagree about where "here" is.
  let focusKey = $state<string | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)

  const focusIndex = (i: number | null) => {
    if (i === null || i < 0 || i >= rows.length) return
    focusKey = keyOf(rows[i]!)
  }
  const indexOfKey = (k: string | null) => (k ? rows.findIndex((r) => keyOf(r) === k) : -1)

  // focusKey changes (arrow keys, healing) drive the REAL focus — the effect
  // finds the row's button and focuses it, scrolling it into view. Reading
  // rows keeps it honest across refreshes without stealing focus on mount:
  // focusKey starts null and only the keyboard (or a click, via onfocusin)
  // sets it.
  $effect(() => {
    void rows
    const k = focusKey
    if (!k || !listEl) return
    const el = listEl.querySelector<HTMLButtonElement>(`[data-row-key="${CSS.escape(k)}"] > button`)
    if (el && document.activeElement !== el) {
      el.focus({ preventScroll: true })
      el.scrollIntoView({ block: 'nearest' })
    }
  })

  // After a list-changing action (delete, move), focus heals to the row that
  // took the changed one's place — never to nothing, never to the top.
  let pendingFocusIndex = $state<number | null>(null)
  $effect(() => {
    void rows
    const p = pendingFocusIndex
    if (p === null) return
    pendingFocusIndex = null
    focusIndex(Math.min(p, rows.length - 1))
  })
  const healFocusFrom = (key: string) => {
    pendingFocusIndex = Math.min(Math.max(indexOfKey(key), 0), rows.length - 1)
  }

  // The checkbox toggle arrives as a `change` event, which carries no modifier
  // state — the pointerdown that preceded it on the same row does. Body clicks
  // pass their own event; the checkbox path reads this.
  let lastPointerShift = false

  // A selection is only meaningful over the rows on screen. Changing place,
  // folder, or filter prunes it rather than leaving invisible items armed for
  // the next Delete. `untrack` on the read: this effect WRITES `selected`, and
  // reading it reactively would make it re-run on its own result.
  let lastSig = ''
  $effect(() => {
    const sig = rows.map(keyOf).join(',')
    if (sig === lastSig) return
    lastSig = sig
    const current = untrack(() => selected)
    if (!current.size) return
    const live = new Set(rows.map(keyOf))
    const next = new Set([...current].filter((k) => live.has(k)))
    if (next.size !== current.size) selected = next
  })

  // The clipboard is pruned exactly like the selection — a deleted item must
  // not stay armed for the next paste.
  $effect(() => {
    const sig = rows.map(keyOf).join(',')
    if (sig !== lastSig) pruneClipboard(new Set(rows.map(keyOf)))
  })

  const selectedRows = $derived(rows.filter((r) => selected.has(keyOf(r))))
  const dragOf = (r: Row): NonNullable<Drag> => {
    // Dragging a row that's part of the selection carries the whole selection;
    // dragging an unselected row carries just it (and takes the selection over).
    const set = selected.has(keyOf(r)) ? selectedRows : [r]
    return { folders: set.filter((x) => x.type === 'folder').map((x) => x.id), artifacts: set.filter((x) => x.type === 'artifact').map((x) => x.id) }
  }

  const toggle = (r: Row, i: number, e?: Event) => {
    // Body clicks carry MouseEvent.shiftKey; a checkbox `change` carries
    // nothing, and the pointerdown that caused it supplied `lastPointerShift`.
    const shift = (e as MouseEvent | undefined)?.shiftKey ?? lastPointerShift
    const next = new Set(selected)
    if (shift && anchor !== null) {
      const [lo, hi] = anchor < i ? [anchor, i] : [i, anchor]
      for (let k = lo; k <= hi; k++) {
        const row = rows[k]
        if (row) next.add(keyOf(row))
      }
    } else {
      const k = keyOf(r)
      if (next.has(k)) next.delete(k)
      else next.add(k)
      anchor = i
    }
    selected = next
  }
  const clear = () => {
    selected = new Set()
    anchor = null
  }

  // ── Clipboard verbs — the selection bar's, the menus', and the keys' ──────
  // After a cut or copy the selection IS the clipboard: clearing it here
  // hands the rows back their normal look and leaves the paste armed.
  const cut = () => {
    if (!selected.size) return
    cutClipboard([...selected], folderId)
    clear()
  }
  const copy = () => {
    if (!selected.size) return
    copyClipboard([...selected], folderId)
    clear()
  }
  const paste = (into?: string) => onPaste(into)

  const duplicate = async (r: Row) => {
    try {
      if (r.type === 'folder') await duplicateFolder(r.id)
      else await duplicateArtifact(r.id)
    } catch (e) {
      pushToast({ title: 'Could not duplicate', body: errorMessage(e), tone: 'danger' })
      return
    }
    await onRefresh()
  }

  // ── Keyboard: the desktop file-manager grammar ─────────────────────────────
  // Selection follows focus (Finder/Explorer): plain arrows move focus AND
  // select just that row; Shift extends from the anchor; ⌘/Ctrl moves focus
  // alone; Space toggles without opening; Enter/→ opens; ← climbs. Grid view
  // trades the horizontal keys for tile steps.

  /** One grid step: the tile at (or nearest in) the focused column, one tile
   *  row up/down. Geometry, not arithmetic — the auto-fill column count is
   *  the stylesheet's business, not ours. */
  const gridStep = (dy: number): void => {
    const i = indexOfKey(focusKey)
    if (!listEl) return
    if (i < 0) {
      // Nothing focused yet — the first arrow keys the first tile, exactly
      // like the list view's moveFocus seeding.
      if (rows.length) focusIndex(0)
      return
    }
    const elts = [...listEl.querySelectorAll<HTMLElement>('[data-row-key]')]
    const from = elts[i]
    if (!from) return
    const top = from.offsetTop
    const band = (t: number) => elts.filter((e) => Math.abs(e.offsetTop - t) < 2)
    if (dy < 0) {
      const above = elts.filter((e) => e.offsetTop < top - 2)
      if (!above.length) return
      const t = Math.max(...above.map((e) => e.offsetTop))
      const row = band(t)
      focusKey = row[Math.min(band(top).indexOf(from), row.length - 1)]?.dataset.rowKey ?? null
    } else {
      const below = elts.filter((e) => e.offsetTop > top + 2)
      if (!below.length) return
      const t = Math.min(...below.map((e) => e.offsetTop))
      const row = band(t)
      focusKey = row[Math.min(band(top).indexOf(from), row.length - 1)]?.dataset.rowKey ?? null
    }
  }

  /** Focus + (unless modifier-held) single-select a row by index; the anchor
   *  follows, so a later Shift+arrow extends from here. */
  const moveFocus = (i: number, e: KeyboardEvent) => {
    const row = rows[i]
    if (!row) return
    focusIndex(i)
    if (e.metaKey || e.ctrlKey) return
    if (e.shiftKey && anchor !== null) {
      const next = new Set(selected)
      const [lo, hi] = anchor < i ? [anchor, i] : [i, anchor]
      for (let k = lo; k <= hi; k++) {
        const r = rows[k]
        if (r) next.add(keyOf(r))
      }
      selected = next
    } else {
      selected = new Set([keyOf(row)])
      anchor = i
    }
  }

  const onKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Escape' && selected.size) clear()
    const t = e.target as HTMLElement | null
    // Never steal a shortcut from a field someone is typing in.
    if (t && (t.isContentEditable || /^(INPUT|TEXTAREA)$/.test(t.tagName))) return
    if ((e.metaKey || e.ctrlKey) && e.key === 'a' && rows.length) {
      e.preventDefault()
      selected = new Set(rows.map(keyOf))
      return
    }
    // ⌘I / Alt+Enter — Get Info and Properties, on the two platforms whose
    // habits a file browser inherits.
    if (selectedRows.length === 1 && (((e.metaKey || e.ctrlKey) && e.key === 'i') || (e.altKey && e.key === 'Enter'))) {
      e.preventDefault()
      onProperties(selectedRows[0]!)
      return
    }
    // A modal owns the keyboard while it is open.
    if (document.querySelector('[role=dialog]')) return
    // The clipboard keys. X and C only intercept with a selection — a bare
    // ⌘C must keep copying text like everywhere else.
    if ((e.metaKey || e.ctrlKey) && (e.key === 'x' || e.key === 'c') && selected.size && canOrganize) {
      e.preventDefault()
      if (e.key === 'x') cut()
      else copy()
      return
    }
    if ((e.metaKey || e.ctrlKey) && e.key === 'v' && clipboard() && canOrganize) {
      e.preventDefault()
      void paste()
      return
    }
    // Escape's ladder: the most transient thing first — a clipboard outlives
    // a selection, so the selection clears before the clipboard does.
    if (e.key === 'Escape') {
      if (selected.size) clear()
      else if (clipboard()) clearClipboard()
      return
    }
    if (!rows.length) return
    const i = indexOfKey(focusKey)
    switch (e.key) {
      case 'ArrowDown': {
        e.preventDefault()
        if (view === 'grid') { gridStep(1); if (!(e.metaKey || e.ctrlKey) && focusKey) moveFocus(Math.max(indexOfKey(focusKey), 0), e) }
        else moveFocus(i < 0 ? 0 : Math.min(i + 1, rows.length - 1), e)
        return
      }
      case 'ArrowUp': {
        e.preventDefault()
        if (view === 'grid') { gridStep(-1); if (!(e.metaKey || e.ctrlKey) && focusKey) moveFocus(Math.max(indexOfKey(focusKey), 0), e) }
        else moveFocus(i < 0 ? rows.length - 1 : Math.max(i - 1, 0), e)
        return
      }
      case 'ArrowRight': {
        if (view === 'grid') { e.preventDefault(); if (i >= 0) focusIndex(Math.min(i + 1, rows.length - 1)) }
        else if (i >= 0) { e.preventDefault(); open(rows[i]!) }
        return
      }
      case 'ArrowLeft': {
        if (view === 'grid') { e.preventDefault(); if (i > 0) focusIndex(i - 1) }
        else { e.preventDefault(); onAscend() }
        return
      }
      case 'Home':
        e.preventDefault()
        moveFocus(0, e)
        return
      case 'End':
        e.preventDefault()
        moveFocus(rows.length - 1, e)
        return
      case ' ':
        if (i >= 0) { e.preventDefault(); toggle(rows[i]!, i) }
        return
      case 'Enter':
        if (i >= 0) { e.preventDefault(); open(rows[i]!) }
        return
    }
  }

  const open = (r: Row) => (r.type === 'folder' ? onOpenFolder(r.id) : onOpenArtifact(r.id))

  const removeRow = async (r: Row) => {
    healFocusFrom(keyOf(r))
    if (r.type === 'folder') {
      if (!(await confirm({ title: 'Delete folder', message: `Delete "${r.name}"? Everything inside moves up a level.`, confirmLabel: 'Delete', danger: true }))) return
      try {
        await deleteFolder(r.id)
      } catch (e) {
        pushToast({ title: 'Delete failed', body: errorMessage(e), tone: 'danger' })
      }
    } else {
      if (!(await confirm({ title: 'Delete file', message: `Delete "${r.name}"?`, confirmLabel: 'Delete', danger: true }))) return
      try {
        await deleteArtifact(r.id)
      } catch (e) {
        pushToast({ title: 'Delete failed', body: errorMessage(e), tone: 'danger' })
        await onRefresh()
        return
      }
      onDeleted(r.id)
    }
    await onRefresh()
  }

  const removeSelection = async () => {
    const n = selectedRows.length
    if (!n) return
    healFocusFrom(keyOf(selectedRows[0]!))
    const label = n === 1 ? `"${selectedRows[0]!.name}"` : `${n} items`
    if (!(await confirm({ title: 'Delete', message: `Delete ${label}? Folders keep their contents, moved up a level.`, confirmLabel: 'Delete', danger: true }))) return
    try {
      for (const r of selectedRows) {
        if (r.type === 'folder') await deleteFolder(r.id)
        else {
          await deleteArtifact(r.id)
          onDeleted(r.id)
        }
      }
    } catch (e) {
      // Stops at the first refusal — earlier deletes stands, later ones don't.
      pushToast({ title: 'Delete failed', body: errorMessage(e), tone: 'danger' })
    }
    clear()
    await onRefresh()
  }

  const rename = async (r: Row) => {
    const name = await prompt({ title: 'Rename folder', defaultValue: r.name, confirmLabel: 'Rename' })
    if (!name?.trim() || name === r.name) return
    try {
      await updateFolder(r.id, { name: name.trim() })
    } catch (e) {
      pushToast({ title: 'Rename failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    await onRefresh()
  }

  const downloadHref = (a: Artifact | null) => (a?.storageRef ? `/api/uploads/${a.storageRef}` : null)

  const rowMenu = (r: Row): ContextMenuEntry[] => {
    const items: ContextMenuEntry[] = [{ label: 'Open', onSelect: () => open(r) }]
    // Share is the second verb for BOTH kinds: "share this folder with the
    // team" is the commonest sharing act there is, and a browser that only
    // shares files makes people move everything to the root to do it.
    items.push({ label: 'Share', icon: [Share2, { size: 13 }], onSelect: () => onShare(r) })
    if (r.type === 'folder') {
      if (canOrganize) items.push({ label: 'Rename', onSelect: () => void rename(r) })
    } else {
      items.push({ label: 'Copy link', onSelect: () => copyAppLink(`/artifacts?a=${r.id}`) })
      const slug = r.artifact?.publicSlug
      if (slug) items.push({ label: 'Copy public link', onSelect: () => copyAppLink(`/a/${slug}`) })
      const href = downloadHref(r.artifact)
      if (href) items.push({ label: 'Download', onSelect: () => window.open(href, '_blank', 'noopener') })
    }
    if (canOrganize) {
      items.push('sep')
      items.push({ label: 'Cut', onSelect: () => { clear(); cutClipboard([keyOf(r)], folderId) } })
      items.push({ label: 'Copy', onSelect: () => { clear(); copyClipboard([keyOf(r)], folderId) } })
      if (clipboard() && r.type === 'folder') {
        items.push({ label: 'Paste into', disabled: !canOrganize, onSelect: () => void paste(r.id) })
      }
      items.push({ label: 'Duplicate', onSelect: () => void duplicate(r) })
      items.push({ label: 'Move to…', onSelect: () => (moveDialog = true) })
    }
    // Properties sits last among the safe actions, the way every file browser
    // puts Get Info / Properties at the foot of the menu.
    items.push({ label: 'Properties', onSelect: () => onProperties(r) })
    items.push('sep', { label: r.type === 'folder' ? 'Delete folder' : 'Delete file', danger: true, onSelect: () => void removeRow(r) })
    return items
  }

  // ── Drops ──────────────────────────────────────────────────────────────────
  // Two different gestures land on the same targets: an INTERNAL row drag
  // (move) and an EXTERNAL file drag from the desktop (upload). `drag` being
  // set is what tells them apart — dataTransfer.types alone can't, because a
  // row drag also carries types.
  const hasFiles = (e: DragEvent) => !drag && !!e.dataTransfer?.types.includes('Files')

  const dropOnFolder = async (e: DragEvent, folderId: string) => {
    e.preventDefault()
    e.stopPropagation()
    overFolder = null
    if (drag) {
      const d = drag
      drag = null
      // A folder cannot be dropped into itself.
      if (d.folders.includes(folderId)) return
      clear()
      await onMove(d, folderId)
      return
    }
    // Files from the desktop dropped ONTO a folder belong IN that folder —
    // stopPropagation means the body handler will never see this drop, so
    // swallowing it here would lose the most natural gesture in the surface.
    const files = Array.from(e.dataTransfer?.files ?? [])
    if (!files.length) return
    uploading = true
    try {
      await onUpload(files, folderId)
    } finally {
      uploading = false
    }
  }

  const dropOnBody = async (e: DragEvent) => {
    e.preventDefault()
    fileOver = false
    const files = Array.from(e.dataTransfer?.files ?? [])
    if (files.length) {
      uploading = true
      try {
        await onUpload(files)
      } finally {
        uploading = false
      }
      return
    }
    if (drag && canOrganize) {
      const d = drag
      drag = null
      clear()
      await onMove(d, null) // the body IS the current folder
    }
  }

  const COLUMNS: { key: SortKey; label: string; class: string }[] = [
    { key: 'name', label: 'Name', class: 'min-w-0' },
    { key: 'kind', label: 'Kind', class: '' },
    { key: 'owner', label: 'Owner', class: '' },
    { key: 'modified', label: 'Modified', class: '' },
  ]
</script>

<svelte:window onkeydown={onKeydown} />

<div
  class="relative flex h-full min-h-0 flex-col"
  role="presentation"
  ondragover={(e) => {
    if (hasFiles(e)) {
      e.preventDefault()
      fileOver = true
    } else if (drag && canOrganize) {
      e.preventDefault()
    }
  }}
  ondragleave={(e) => {
    // Only the real exit counts — dragging across a child fires dragleave too.
    if (e.currentTarget === e.target) fileOver = false
  }}
  ondrop={(e) => void dropOnBody(e)}
  oncontextmenu={(e) => {
    // The empty-space menu mirrors the surface's own verbs — one today: paste.
    if ((e.target as HTMLElement).closest('[data-row-key]')) return
    e.preventDefault()
    menu.openMenu(e, [
      { label: 'Paste', disabled: !clipboard() || !canOrganize, onSelect: () => void paste() },
    ])
  }}
>
  {#if view === 'list' && (rows.length > 0 || loading)}
    <!-- Column heads sit outside the scroller so they stay put; ROW_GRID is
         the one shared template (header + rows), which keeps them aligned as
         the pane resizes. Track 1 is the selection column: the select-all
         checkbox — ⌘A's visible twin. -->
    <div class={cn('grid shrink-0 items-center gap-3 border-b border-line-subtle px-2 pb-1.5', ROW_GRID)}>
      <Checkbox
        bare
        title="Select all"
        checked={rows.length > 0 && rows.every((r) => selected.has(keyOf(r)))}
        onChange={(checked) => {
          selected = checked ? new Set(rows.map(keyOf)) : new Set()
          anchor = null
        }}
      />
      {#each COLUMNS as c (c.key)}
        <button
          type="button"
          onclick={() => onSort(c.key)}
          class={cn('flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.08em] transition-colors hover:text-fg', sortKey === c.key ? 'text-fg' : 'text-ink-dim', c.class)}
        >
          {c.label}
          {#if sortKey === c.key}
            {#if sortDir === 'asc'}<ArrowUp size={10} />{:else}<ArrowDown size={10} />{/if}
          {/if}
        </button>
      {/each}
    </div>
  {/if}

  <div class="min-h-0 flex-1 overflow-y-auto px-2 py-2">
    {#if loading}
      <SkeletonRows rows={8} class="py-1" />
    {:else if failure && rows.length === 0}
      <div in:fade={{ duration: 150 }}>
        <QueryError error={failure.error} title={failure.title} onRetry={failure.retry} />
      </div>
    {:else if rows.length === 0}
      <div in:fade={{ duration: 150 }}>
        <EmptyState icon="◆" title={emptyTitle} hint={emptyHint} />
      </div>
    {:else}
      <!-- Icon view packs tight (§ desktop idiom): many small cells per row,
           hairline gaps, so a folder reads as a field of files at a glance. -->
      <div bind:this={listEl} class={cn(view === 'grid' && 'grid grid-cols-[repeat(auto-fill,minmax(6.5rem,1fr))] gap-0.5')} use:listStagger>
        {#each rows as r, i (keyOf(r))}
          {@const k = keyOf(r)}
          {@const shared = {
            row: r,
            rowKey: k,
            selected: selected.has(k),
            cut: isCut(k),
            focused: focusKey === k,
            active: r.type === 'artifact' && r.id === activeId,
            dropTarget: overFolder === r.id,
            onOpen: () => open(r),
            onToggle: (e?: Event) => toggle(r, i, e ?? undefined),
            onFocusIn: () => (focusKey = k),
            onPointerDown: (e: PointerEvent) => (lastPointerShift = e.shiftKey),
            onContextMenu: (e: MouseEvent) => {
              if (!selected.has(k)) clear()
              menu.openMenu(e, rowMenu(r))
            },
            ondragstart: (e: DragEvent) => {
              e.stopPropagation()
              if (!selected.has(keyOf(r))) selected = new Set([keyOf(r)])
              const payload = dragOf(r)
              drag = payload
              if (e.dataTransfer) {
                e.dataTransfer.effectAllowed = 'move'
                // Also on the dataTransfer, so the breadcrumb can take a drop.
                e.dataTransfer.setData(DRAG_MIME, JSON.stringify(payload))
              }
            },
            ondragend: () => {
              drag = null
              overFolder = null
            },
            ...(r.type === 'folder'
              ? {
                  ondragover: (e: DragEvent) => {
                    const external = !drag && !!e.dataTransfer?.types.includes('Files')
                    if (!external && (!drag || !canOrganize || drag.folders.includes(r.id))) return
                    e.preventDefault()
                    e.stopPropagation()
                    overFolder = r.id
                  },
                  ondragleave: () => (overFolder = overFolder === r.id ? null : overFolder),
                  ondrop: (e: DragEvent) => void dropOnFolder(e, r.id),
                }
              : {}),
          }}
          {#if view === 'grid'}
            <ArtifactsTile {...shared} />
          {:else}
            <ArtifactsRow {...shared} />
          {/if}
        {/each}
      </div>
      {#if failure}
        <!-- Half the tree answered. Keep what loaded and say so — swapping a
             populated pane for an error loses more than it explains. -->
        <QueryError variant="inline" class="px-2 py-3" error={failure.error} title={failure.title} onRetry={failure.retry} />
      {/if}
    {/if}
  </div>

  {#if selected.size > 0}
    <!-- Selection bar: the actions that only make sense for MANY things at
         once. Moving stays a drag; Delete is a quiet link, never a button. -->
    <div transition:fade={{ duration: 120 }} class="flex shrink-0 items-center gap-3 border-t border-line-subtle bg-panel px-4 py-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-fg">{selected.size} selected</span>
      {#if selectedRows.length === 1 && downloadHref(selectedRows[0]!.artifact)}
        <a href={downloadHref(selectedRows[0]!.artifact)} target="_blank" rel="noreferrer" class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted underline-offset-2 transition-colors hover:text-fg hover:underline">
          Download
        </a>
      {/if}
      {#if canOrganize}
        <Button variant="ghost" size="xs" onclick={cut}>Cut</Button>
        <Button variant="ghost" size="xs" onclick={copy}>Copy</Button>
        <Button variant="ghost" size="xs" onclick={() => (moveDialog = true)}>Move</Button>
      {/if}
      <DangerLink onClick={() => void removeSelection()}>Delete</DangerLink>
      <Button variant="ghost" size="xs" class="ml-auto" onclick={clear}>
        Clear
      </Button>
    </div>
  {/if}

  {#if (fileOver && !overFolder) || uploading}
    <!-- Drop-to-upload is the affordance a tree never had: the whole pane is
         the target, and it says so only while something is over it. -->
    <div transition:fade={{ duration: 120 }} class="pointer-events-none absolute inset-2 z-20 grid place-items-center rounded-xl border-2 border-dashed border-[var(--theme-accent-border)] bg-surface/80">
      <div class="flex flex-col items-center gap-2 text-center">
        <Upload size={22} class="text-accent" />
        <div class="font-sans text-sm text-fg">{uploading ? 'Uploading' : 'Drop to upload'}</div>
        <div class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">Up to 25 MB each · stored and hosted by Talaria</div>
      </div>
    </div>
  {/if}

  <MoveDialog
    open={moveDialog}
    onClose={() => (moveDialog = false)}
    selection={selected.size
      ? { folders: selectedRows.filter((r) => r.type === 'folder').map((r) => r.id), artifacts: selectedRows.filter((r) => r.type === 'artifact').map((r) => r.id) }
      : { folders: [], artifacts: [] }}
    startAt={folderId}
    onMove={async (target) => {
      await onMove(
        {
          folders: selectedRows.filter((r) => r.type === 'folder').map((r) => r.id),
          artifacts: selectedRows.filter((r) => r.type === 'artifact').map((r) => r.id),
        },
        target,
      )
      clear()
    }}
  />
  <ContextMenu {menu} />
</div>
