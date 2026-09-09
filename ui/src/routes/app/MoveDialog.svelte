<script lang="ts">
  import { ChevronRight, Folder, FolderPlus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { cn } from '@/lib/cn'
  import { pushToast } from '@/lib/toast.svelte'
  import { createFolder, useFolders } from '@/lib/artifacts'
  import { errorMessage } from '@/lib/fetch-json'
  import { ancestry, type Drag } from './artifacts'

  // The move dialog — a folder browser, not a dropdown. Navigate in (click a
  // row), climb back out (crumbs), make a new folder on the spot, Move here.
  // The same grammar as the Files list itself: folders are destinations,
  // files never appear, and a folder cannot move into itself or its own
  // descendants (those rows are dimmed and refuse navigation).
  let {
    open,
    onClose,
    selection,
    startAt,
    onMove,
  }: {
    open: boolean
    onClose: () => void
    /** What is moving — the browser's Drag payload shape. */
    selection: NonNullable<Drag>
    /** The folder the dialog opens standing in (usually the current one). */
    startAt: string | null
    /** Receives the chosen destination; null = the root. */
    onMove: (target: string | null) => Promise<unknown>
  } = $props()

  const foldersQuery = useFolders()
  const folders = $derived(foldersQuery.data ?? [])

  let standing = $state<string | null>(null)
  let moving = $state(false)
  let creating = $state(false)
  let newName = $state('')

  // Re-seed the standing folder each time the dialog opens — the dialog is a
  // picker, not a place; its navigation state never outlives a session with it.
  $effect(() => {
    if (open) {
      standing = startAt
      creating = false
      newName = ''
    }
  })

  const trail = $derived(ancestry(standing, folders))
  const children = $derived(
    folders.filter((f) => (f.parentId ?? null) === standing).sort((a, b) => a.name.localeCompare(b.name)),
  )

  // A folder may not move into itself or any of its descendants — the set the
  // move would corrupt. Computed by walking down from every selected folder,
  // bounded like every walk in this view (the table tolerates cycles; the
  // picker must not inherit one).
  const blocked = $derived.by(() => {
    const out = new Set<string>(selection.folders)
    let grew = true
    while (grew) {
      grew = false
      for (const f of folders) {
        const parent = f.parentId ?? null
        if (parent !== null && out.has(parent) && !out.has(f.id)) {
          out.add(f.id)
          grew = true
        }
      }
    }
    return out
  })

  const step = (id: string) => {
    if (!blocked.has(id)) standing = id
  }

  const makeFolder = async () => {
    const name = newName.trim()
    if (!name) return
    try {
      const folder = await createFolder(name, standing)
      await foldersQuery.refetch()
      creating = false
      newName = ''
      if (!blocked.has(folder.id)) standing = folder.id
    } catch (e) {
      pushToast({ title: 'Could not create the folder', body: errorMessage(e), tone: 'danger' })
    }
  }

  const move = async () => {
    moving = true
    try {
      await onMove(standing)
    } finally {
      moving = false
      onClose()
    }
  }
</script>

<Modal {open} {onClose} title="Move" width="max-w-lg" padded={false}>
  <div class="flex max-h-[60vh] min-h-[18rem] flex-col">
    <!-- Crumbs: the walked path. Each is a step back out; the root crumb is
         the top of My Files. -->
    <div class="flex min-h-9 flex-wrap items-center gap-0.5 border-b border-line-subtle px-4 py-2">
      <button
        type="button"
        onclick={() => (standing = null)}
        class={cn('rounded px-1.5 py-0.5 font-sans text-sm font-semibold transition-colors', standing ? 'text-muted hover:text-fg' : 'text-fg')}
      >
        My Files
      </button>
      {#each trail as f (f.id)}
        <ChevronRight size={13} class="shrink-0 text-ink-dim" />
        <button
          type="button"
          onclick={() => (standing = f.id)}
          class={cn('min-w-0 truncate rounded px-1.5 py-0.5 font-sans text-sm font-semibold transition-colors', f.id === standing ? 'text-fg' : 'text-muted hover:text-fg')}
        >
          {f.name}
        </button>
      {/each}
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto p-2">
      {#if children.length === 0}
        <div class="grid h-full place-items-center px-4 py-8 font-sans text-sm text-muted">
          No folders inside. Move here, or make one below.
        </div>
      {:else}
        {#each children as f (f.id)}
          {@const isBlocked = blocked.has(f.id)}
          <button
            type="button"
            disabled={isBlocked}
            onclick={() => step(f.id)}
            class={cn(
              'flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors',
              isBlocked ? 'cursor-not-allowed opacity-40' : 'dither-fill hover:bg-raised/60',
              standing === f.id && 'bg-card',
            )}
          >
            <Folder size={15} class="shrink-0 text-accent" />
            <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{f.name}</span>
            {#if isBlocked}
              <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">can’t move into itself</span>
            {:else}
              <ChevronRight size={13} class="shrink-0 text-ink-dim" />
            {/if}
          </button>
        {/each}
      {/if}
    </div>
  </div>

  {#snippet footer()}
    <div class="flex w-full items-center gap-2">
      {#if creating}
        <Input
          autofocus
          size="sm"
          class="h-7 w-44"
          placeholder="folder name"
          bind:value={newName}
          onkeydown={(e) => {
            if (e.key === 'Enter') void makeFolder()
            else if (e.key === 'Escape') { creating = false; newName = '' }
          }}
          onblur={() => void makeFolder()}
        />
      {:else}
        <Button variant="ghost" size="xs" onclick={() => (creating = true)}>
          <FolderPlus size={13} />
          New folder
        </Button>
      {/if}
      <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
        {selection.folders.length + selection.artifacts.length} item{selection.folders.length + selection.artifacts.length === 1 ? '' : 's'}
      </span>
      <Button variant="ghost" size="sm" onclick={onClose}>Cancel</Button>
      <Button size="sm" disabled={moving || blocked.has(standing ?? '')} onclick={() => void move()}>
        {moving ? 'Moving…' : 'Move here'}
      </Button>
    </div>
  {/snippet}
</Modal>
