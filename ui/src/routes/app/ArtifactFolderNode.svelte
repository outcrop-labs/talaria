<script lang="ts">
  import ArtifactFolderNode from './ArtifactFolderNode.svelte'
  import { ChevronRight, Folder } from '@lucide/svelte'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { deleteFolder, updateFolder, type Artifact, type ArtifactFolder } from '@/lib/artifacts'
  import ArtifactRow from './ArtifactRow.svelte'
  import type { Drag } from './artifacts'

  let {
    folder,
    depth,
    foldersByParent,
    byFolder,
    expanded,
    onToggle,
    activeId,
    onSelect,
    drag,
    setDrag,
    onDrop,
    onRefresh,
    openMenu,
    onArtifactMenu,
    onNewFolder,
  }: {
    folder: ArtifactFolder
    depth: number
    foldersByParent: Map<string | null, ArtifactFolder[]>
    byFolder: Map<string | null, Artifact[]>
    expanded: Set<string>
    /** Toggle a folder open/closed (was React's setExpanded dispatch). */
    onToggle: (id: string) => void
    activeId: string | null
    onSelect: (id: string) => void
    drag: Drag
    setDrag: (d: Drag) => void
    onDrop: (folderId: string | null) => void
    onRefresh: () => Promise<unknown>
    openMenu: (e: MouseEvent, items: ContextMenuEntry[]) => void
    onArtifactMenu: (e: MouseEvent, a: Artifact) => void
    onNewFolder: (parentId: string) => void
  } = $props()

  let over = $state(false)
  let renaming = $state(false)
  let name = $state(folder.name)
  $effect(() => {
    name = folder.name
  })
  const isOpen = $derived(expanded.has(folder.id))
  const childFolders = $derived(foldersByParent.get(folder.id) ?? [])
  const childArtifacts = $derived(byFolder.get(folder.id) ?? [])
  const toggle = () => onToggle(folder.id)
  // Same confirm + deleteFolder flow as the row's ✕ button.
  const remove = async () => {
    if (await confirm({ title: 'Delete folder', message: `Delete folder "${folder.name}"? Its artifacts move to the top level.`, confirmLabel: 'Delete', danger: true })) {
      await deleteFolder(folder.id)
      await onRefresh()
    }
  }
  const folderMenu = (): ContextMenuEntry[] => [
    { label: 'Rename', onSelect: () => (renaming = true) },
    { label: 'New folder inside', onSelect: () => onNewFolder(folder.id) },
    'sep',
    { label: 'Delete folder', danger: true, onSelect: () => void remove() },
  ]
</script>

<div>
  <div
    draggable="true"
    ondragstart={(e) => {
      e.stopPropagation()
      setDrag({ kind: 'folder', id: folder.id })
    }}
    ondragend={() => setDrag(null)}
    ondragover={(e) => {
      e.preventDefault()
      e.stopPropagation()
      if (drag && !(drag.kind === 'folder' && drag.id === folder.id)) over = true
    }}
    ondragleave={() => (over = false)}
    ondrop={(e) => {
      e.preventDefault()
      e.stopPropagation()
      over = false
      onDrop(folder.id)
    }}
    oncontextmenu={renaming ? undefined : (e) => openMenu(e, folderMenu())}
    class={cn('group flex items-center gap-1 rounded-md py-1 pr-1 text-sm text-muted transition-colors hover:bg-hover hover:text-fg', over && 'ring-1 ring-accent/60')}
    style:padding-left="{depth * 14 + 2}px"
  >
    <button type="button" onclick={toggle} class={cn('shrink-0 rounded p-0.5 hover:bg-card2', childFolders.length + childArtifacts.length === 0 && 'invisible')}>
      <ChevronRight size={12} class={cn('transition-transform', isOpen && 'rotate-90')} />
    </button>
    {#if renaming}
      <Input
        size="sm"
        autofocus
        bind:value={name}
        onblur={async () => {
          renaming = false
          if (name.trim() && name !== folder.name) {
            await updateFolder(folder.id, { name: name.trim() })
            await onRefresh()
          }
        }}
        onkeydown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur()
          if (e.key === 'Escape') {
            name = folder.name
            renaming = false
          }
        }}
        class="h-6 flex-1"
      />
    {:else}
      <button type="button" onclick={toggle} ondblclick={() => (renaming = true)} class="flex min-w-0 flex-1 items-center gap-1.5 text-left">
        <span class="grid w-4 shrink-0 place-items-center">{#if folder.icon}{folder.icon}{:else}<Folder size={13} />{/if}</span>
        <span class="truncate font-medium">{folder.name}</span>
      </button>
    {/if}
    <CloseButton
      size={12}
      label="Delete folder"
      onClick={() => void remove()}
      class="shrink-0 rounded p-0.5 opacity-0 hover:bg-transparent hover:text-danger group-hover:opacity-100"
    />
  </div>
  {#if isOpen}
    <div>
      {#each childFolders as f (f.id)}
        <ArtifactFolderNode folder={f} depth={depth + 1} {foldersByParent} {byFolder} {expanded} {onToggle} {activeId} {onSelect} {drag} {setDrag} {onDrop} {onRefresh} {openMenu} {onArtifactMenu} {onNewFolder} />
      {/each}
      {#each childArtifacts as a (a.id)}
        <ArtifactRow artifact={a} depth={depth + 1} {activeId} {onSelect} {setDrag} onContextMenu={(e) => onArtifactMenu(e, a)} />
      {/each}
    </div>
  {/if}
</div>
