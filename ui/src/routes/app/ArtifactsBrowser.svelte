<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { untrack } from 'svelte'
  import { ArrowDown, ArrowUp, Share2, Upload } from '@lucide/svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { copyAppLink, useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { confirm, prompt } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { fade, listStagger } from '@/lib/motion'
  import { deleteArtifact, deleteFolder, updateFolder, type Artifact } from '@/lib/artifacts'
  import ArtifactsRow from './ArtifactsRow.svelte'
  import ArtifactsTile from './ArtifactsTile.svelte'
  import { DRAG_MIME, type Drag, type Row, type SortDir, type SortKey } from './artifacts'

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
    emptyTitle,
    emptyHint,
    onOpenFolder,
    onOpenArtifact,
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
    emptyTitle: string
    emptyHint?: string
    onOpenFolder: (id: string) => void
    onOpenArtifact: (id: string) => void
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

  const selectedRows = $derived(rows.filter((r) => selected.has(keyOf(r))))
  const dragOf = (r: Row): NonNullable<Drag> => {
    // Dragging a row that's part of the selection carries the whole selection;
    // dragging an unselected row carries just it (and takes the selection over).
    const set = selected.has(keyOf(r)) ? selectedRows : [r]
    return { folders: set.filter((x) => x.type === 'folder').map((x) => x.id), artifacts: set.filter((x) => x.type === 'artifact').map((x) => x.id) }
  }

  const toggle = (r: Row, i: number, e: MouseEvent) => {
    const next = new Set(selected)
    if (e.shiftKey && anchor !== null) {
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

  const open = (r: Row) => (r.type === 'folder' ? onOpenFolder(r.id) : onOpenArtifact(r.id))

  const removeRow = async (r: Row) => {
    if (r.type === 'folder') {
      if (!(await confirm({ title: 'Delete folder', message: `Delete "${r.name}"? Everything inside moves up a level.`, confirmLabel: 'Delete', danger: true }))) return
      await deleteFolder(r.id)
    } else {
      if (!(await confirm({ title: 'Delete file', message: `Delete "${r.name}"?`, confirmLabel: 'Delete', danger: true }))) return
      await deleteArtifact(r.id)
      onDeleted(r.id)
    }
    await onRefresh()
  }

  const removeSelection = async () => {
    const n = selectedRows.length
    if (!n) return
    const label = n === 1 ? `"${selectedRows[0]!.name}"` : `${n} items`
    if (!(await confirm({ title: 'Delete', message: `Delete ${label}? Folders keep their contents, moved up a level.`, confirmLabel: 'Delete', danger: true }))) return
    for (const r of selectedRows) {
      if (r.type === 'folder') await deleteFolder(r.id)
      else {
        await deleteArtifact(r.id)
        onDeleted(r.id)
      }
    }
    clear()
    await onRefresh()
  }

  const rename = async (r: Row) => {
    const name = await prompt({ title: 'Rename folder', defaultValue: r.name, confirmLabel: 'Rename' })
    if (!name?.trim() || name === r.name) return
    await updateFolder(r.id, { name: name.trim() })
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

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape' && selected.size) clear()
    const t = e.target as HTMLElement | null
    // Never steal a shortcut from a field someone is typing in.
    if (t && (t.isContentEditable || /^(INPUT|TEXTAREA)$/.test(t.tagName))) return
    if ((e.metaKey || e.ctrlKey) && e.key === 'a' && rows.length) {
      e.preventDefault()
      selected = new Set(rows.map(keyOf))
    }
    // ⌘I / Alt+Enter — Get Info and Properties, on the two platforms whose
    // habits a file browser inherits.
    if (selectedRows.length === 1 && (((e.metaKey || e.ctrlKey) && e.key === 'i') || (e.altKey && e.key === 'Enter'))) {
      e.preventDefault()
      onProperties(selectedRows[0]!)
    }
  }}
/>

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
>
  {#if view === 'list' && (rows.length > 0 || loading)}
    <!-- Column heads sit outside the scroller so they stay put; the grid
         template is duplicated in ArtifactsRow, which is what keeps them
         aligned as the pane resizes. -->
    <div class="grid shrink-0 grid-cols-[minmax(0,1fr)_7rem_10rem_8rem] items-center gap-3 border-b border-line-subtle px-2 pb-1.5 pl-9">
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
      <div class={cn(view === 'grid' && 'grid grid-cols-[repeat(auto-fill,minmax(6.5rem,1fr))] gap-0.5')} use:listStagger>
        {#each rows as r, i (keyOf(r))}
          {@const shared = {
            row: r,
            selected: selected.has(keyOf(r)),
            anySelected: selected.size > 0,
            active: r.type === 'artifact' && r.id === activeId,
            dropTarget: overFolder === r.id,
            onOpen: () => open(r),
            onToggle: (e: MouseEvent) => toggle(r, i, e),
            onContextMenu: (e: MouseEvent) => {
              if (!selected.has(keyOf(r))) clear()
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

  <ContextMenu {menu} />
</div>
