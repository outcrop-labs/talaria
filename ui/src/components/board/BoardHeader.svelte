<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Settings2, Archive } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { renameBoard, useBoardMembers, type Board } from '@/lib/boards.svelte'

  // Board page header: editable name, stacked member avatars, and a single settings
  // gear (everything else lives in the board settings modal).
  let { board, onSettings }: { board: Board; onSettings: () => void } = $props()

  const qc = useQueryClient()
  // `{ data: members = [] }` turned a 500 on /api/boards/:id/members into an
  // empty avatar stack — byte-identical to a board only you can see. Who has
  // access is a security-shaped claim; it may only be made from a read that
  // actually returned.
  const membersQuery = useBoardMembers(() => board.id)
  const members = $derived(membersQuery.data ?? [])
  const membersLoading = $derived(membersQuery.isLoading)
  const membersFailed = $derived(membersQuery.isError && membersQuery.data === undefined)
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  let name = $state(board.name)
  let editing = $state(false)

  const commit = async () => {
    editing = false
    const n = name.trim()
    if (!n || n === board.name) {
      name = board.name
      return
    }
    await renameBoard(board.id, n)
    void qc.invalidateQueries({ queryKey: ['boards'] })
  }
</script>

<div class="flex items-center gap-3 border-b border-line-subtle px-5 py-3">
  {#if board.archivedAt}
    <span class="flex items-center gap-1 rounded-md border border-line bg-raised px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
      <Archive size={12} /> Archived
    </span>
  {/if}
  {#if editing}
    <!-- svelte-ignore a11y_autofocus -->
    <input
      autofocus
      bind:value={name}
      onblur={commit}
      onkeydown={(e) => {
        if (e.key === 'Enter') commit()
      }}
      class="min-w-0 flex-1 border-0 bg-transparent font-sans text-lg font-semibold text-fg outline-none"
    />
  {:else}
    <button
      type="button"
      onclick={() => {
        if (canEdit && !board.archivedAt) editing = true
      }}
      class="min-w-0 flex-1 truncate text-left font-sans text-lg font-semibold text-fg"
      title={canEdit ? 'Rename' : undefined}
    >
      {board.name}
    </button>
  {/if}

  {#if membersFailed}
    <QueryError
      variant="inline"
      class="shrink-0 text-right"
      title="Members unavailable"
      error={membersQuery.error}
      onRetry={() => void membersQuery.refetch()}
    />
  {/if}

  <div class="flex -space-x-2">
    {#if membersLoading}
      {#each [0, 1] as i (i)}
        <Skeleton class="h-7 w-7 rounded-full ring-2 ring-[color:var(--theme-panel)]" />
      {/each}
    {/if}
    {#each members.slice(0, 5) as m (m.userId)}
      <Avatar name={m.email ?? m.name} class="h-7 w-7 ring-2 ring-[color:var(--theme-panel)]" />
    {/each}
    {#if members.length > 5}
      <span class="grid h-7 w-7 place-items-center rounded-full border border-line-strong bg-raised font-mono text-[10px] tracking-[0.05em] text-muted ring-2 ring-[color:var(--theme-panel)]">
        +{members.length - 5}
      </span>
    {/if}
  </div>

  {#if canEdit}
    <button data-dither-fill
      type="button"
      onclick={onSettings}
      class="grid h-8 w-8 place-items-center rounded-md text-muted transition-colors hover:text-fg"
      title="Board settings"
      aria-label="Board settings"
    >
      <Settings2 size={17} />
    </button>
  {/if}
</div>
