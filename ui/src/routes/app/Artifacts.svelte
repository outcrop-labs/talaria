<script lang="ts">
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { DownloadCloud, FolderPlus, Plus } from '@lucide/svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { copyAppLink, useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { popPanel } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { fade, listStagger, pop, POPOVER, QUICK } from '@/lib/motion'
  import { createArtifact, createFolder, deleteArtifact, saveArtifact, updateFolder, useArtifacts, useFolders, type Artifact, type ArtifactFolder, type ArtifactKind } from '@/lib/artifacts'
  import ArtifactEditor from './ArtifactEditor.svelte'
  import ArtifactFolderNode from './ArtifactFolderNode.svelte'
  import ArtifactRow from './ArtifactRow.svelte'
  import ArtifactsDriveImportModal from './ArtifactsDriveImportModal.svelte'
  import { NEW_KINDS, type Drag } from './artifacts'

  const qc = useQueryClient()
  // Two reads build one tree, so both rejections have to survive to the render:
  // "No artifacts yet." is a claim about the owner's WORK, and a store that is
  // merely unreachable must never be reported as a store that is empty.
  const artifactsQuery = useArtifacts()
  const foldersQuery = useFolders()
  const artifacts = $derived(artifactsQuery.data ?? [])
  const folders = $derived(foldersQuery.data ?? [])
  // Whichever half broke, in the words the reader needs. Null = both answered.
  const treeFailure = $derived(
    artifactsQuery.isError && artifactsQuery.data === undefined
      ? { title: 'Could not load your artifacts', error: artifactsQuery.error, retry: () => void artifactsQuery.refetch() }
      : foldersQuery.isError && foldersQuery.data === undefined
        ? { title: 'Could not load your folders', error: foldersQuery.error, retry: () => void foldersQuery.refetch() }
        : null,
  )
  // ?a=<artifactId> deep-links an artifact — the URL IS the selection.
  // searchParams.get parses values (string | number | boolean | null); ids are
  // strings, so normalize back before comparing/passing down.
  const rawActive = $derived(searchParams.get('a'))
  const activeId = $derived(rawActive ? String(rawActive) : null)
  const setActiveId = (id: string | null) => {
    if (id) searchParams.set('a', id)
    else searchParams.delete('a')
  }
  let newOpen = $state(false)
  let importOpen = $state(false)
  let expanded = $state<Set<string>>(new Set())
  let drag = $state<Drag>(null)

  const byFolder = $derived.by(() => {
    const m = new Map<string | null, Artifact[]>()
    for (const a of artifacts) {
      const k = a.folderId ?? null
      if (!m.has(k)) m.set(k, [])
      m.get(k)!.push(a)
    }
    return m
  })
  const foldersByParent = $derived.by(() => {
    const m = new Map<string | null, ArtifactFolder[]>()
    for (const f of folders) {
      const k = f.parentId ?? null
      if (!m.has(k)) m.set(k, [])
      m.get(k)!.push(f)
    }
    for (const list of m.values()) list.sort((a, b) => a.name.localeCompare(b.name))
    return m
  })

  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: ['artifacts'] }), qc.invalidateQueries({ queryKey: ['artifact-folders'] })])
  const create = async (kind: ArtifactKind) => {
    newOpen = false
    const { artifact } = await createArtifact({ kind, title: 'Untitled' })
    await qc.invalidateQueries({ queryKey: ['artifacts'] })
    if (artifact) setActiveId(artifact.id)
  }
  const newFolder = async (parentId: string | null = null) => {
    const { folder } = await createFolder('New folder', parentId)
    await qc.invalidateQueries({ queryKey: ['artifact-folders'] })
    if (folder) {
      const n = new Set(expanded).add(folder.id)
      if (parentId) n.add(parentId) // reveal the new subfolder
      expanded = n
    }
  }
  const toggleExpanded = (id: string) => {
    const n = new Set(expanded)
    if (n.has(id)) n.delete(id)
    else n.add(id)
    expanded = n
  }

  // Right-click menus — shortcuts to actions the tree and editor already offer.
  const menu = useContextMenu()
  const artifactMenu = (a: Artifact): ContextMenuEntry[] => {
    const items: ContextMenuEntry[] = [
      { label: 'Open', onSelect: () => setActiveId(a.id) },
      { label: 'Copy link', onSelect: () => copyAppLink(`/artifacts?a=${a.id}`) },
    ]
    const slug = a.publicSlug
    if (slug) items.push({ label: 'Copy public link', onSelect: () => copyAppLink(`/a/${slug}`) })
    items.push('sep', {
      label: 'Delete artifact',
      danger: true,
      onSelect: async () => {
        // Same confirm + deleteArtifact flow as the editor's kebab menu.
        if (!(await confirm({ title: 'Delete artifact', message: `Delete "${a.title}"?`, confirmLabel: 'Delete', danger: true }))) return
        await deleteArtifact(a.id)
        await qc.invalidateQueries({ queryKey: ['artifacts'] })
        if (activeId === a.id) setActiveId(null)
      },
    })
    return items
  }
  // Drop the dragged item into a folder (or root when folderId is null).
  const drop = async (folderId: string | null) => {
    if (!drag) return
    if (drag.kind === 'artifact') await saveArtifact(drag.id, { folderId })
    else await updateFolder(drag.id, { parentId: folderId })
    drag = null
    await refresh()
  }

  const rootArtifacts = $derived(byFolder.get(null) ?? [])
  const rootFolders = $derived(foldersByParent.get(null) ?? [])
</script>

<div class="flex h-full min-h-0">
  <aside class="flex h-full w-72 shrink-0 flex-col border-r border-line-subtle bg-sidebar font-sans">
    <div class="relative flex h-12 shrink-0 items-center gap-1.5 border-b border-line-subtle px-4">
      <span class="min-w-0 flex-1 truncate text-sm font-semibold text-fg">Artifacts</span>
      <div class="flex items-center gap-0.5">
        <IconButton size="sm" title="Import from Google Drive" onclick={() => (importOpen = true)}>
          <DownloadCloud size={15} />
        </IconButton>
        <IconButton size="sm" title="New folder" onclick={() => void newFolder()}>
          <FolderPlus size={15} />
        </IconButton>
        <IconButton size="sm" title="New artifact" onclick={() => (newOpen = !newOpen)} active={newOpen}>
          <Plus size={15} />
        </IconButton>
      </div>
      {#if newOpen}
        <div
          in:pop={POPOVER}
          out:fade={QUICK}
          class={cn(popPanel, 'absolute right-3 top-full z-30 mt-1 w-44 origin-top-right')}
          onmouseleave={() => (newOpen = false)}
          role="menu"
          tabindex="-1"
        >
          {#each NEW_KINDS as { kind, label, icon: Icon } (kind)}
            <button type="button" onclick={() => void create(kind)} class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs text-fg transition-colors hover:bg-hover">
              <Icon size={13} /> {label}
            </button>
          {/each}
        </div>
      {/if}
    </div>
    <div
      class="min-h-0 flex-1 overflow-y-auto p-2"
      ondragover={(e) => e.preventDefault()}
      ondrop={(e) => {
        e.preventDefault()
        void drop(null)
      }}
      role="tree"
      tabindex="-1"
    >
      {#if artifactsQuery.isLoading || foldersQuery.isLoading}
        <!-- Both queries feed the same tree — reveal it once, fully formed. -->
        <SkeletonRows rows={6} class="px-2 py-3" />
      {:else if treeFailure && folders.length === 0 && artifacts.length === 0}
        <div in:fade={{ duration: 150 }}>
          <QueryError variant="compact" error={treeFailure.error} title={treeFailure.title} onRetry={treeFailure.retry} />
        </div>
      {:else if folders.length === 0 && artifacts.length === 0}
        <div in:fade={{ duration: 150 }}>
          <EmptyState variant="inline" title="No artifacts yet." class="px-2 py-6 text-center" />
        </div>
      {:else}
        <!-- Any grid or list staggers its items on mount (ANIMATIONS.md). One
             wrapper over both {#each} runs — folders then loose artifacts —
             so a single cascade owns the tree. It mounts only once both reads
             RESOLVED — never on the skeleton branch. An expanded folder's
             nested rows ride their parent row's rise (direct children only). -->
        <div use:listStagger>
        {#each rootFolders as f (f.id)}
          <ArtifactFolderNode
            folder={f}
            depth={0}
            {foldersByParent}
            {byFolder}
            {expanded}
            onToggle={toggleExpanded}
            {activeId}
            onSelect={setActiveId}
            {drag}
            setDrag={(d) => (drag = d)}
            onDrop={drop}
            onRefresh={refresh}
            openMenu={menu.openMenu}
            onArtifactMenu={(e, a) => menu.openMenu(e, artifactMenu(a))}
            onNewFolder={(parentId) => void newFolder(parentId)}
          />
        {/each}
        {#each rootArtifacts as a (a.id)}
          <ArtifactRow artifact={a} depth={0} {activeId} onSelect={setActiveId} setDrag={(d) => (drag = d)} onContextMenu={(e) => menu.openMenu(e, artifactMenu(a))} />
        {/each}
        </div>
        <!-- One half answered, the other didn't. Keep what loaded and say
             the tree is INCOMPLETE — replacing a populated pane over a
             partial failure loses more than it explains. -->
        {#if treeFailure}
          <QueryError
            variant="inline"
            class="px-2 py-3"
            error={treeFailure.error}
            title={treeFailure.title}
            onRetry={treeFailure.retry}
          />
        {/if}
      {/if}
    </div>
  </aside>
  <main class="min-h-0 min-w-0 flex-1">
    {#if activeId}
      {#key activeId}
        <ArtifactEditor id={activeId} onDeleted={() => setActiveId(null)} />
      {/key}
    {:else}
      <EmptyState icon="◆" title="Artifacts" hint="Create an artifact or a folder. Drag artifacts into folders to organize." />
    {/if}
  </main>
  {#if importOpen}
    <ArtifactsDriveImportModal
      onClose={() => (importOpen = false)}
      onImported={async (artifactId) => {
        importOpen = false
        await qc.invalidateQueries({ queryKey: ['artifacts'] })
        setActiveId(artifactId)
      }}
    />
  {/if}
  <ContextMenu {menu} />
</div>
