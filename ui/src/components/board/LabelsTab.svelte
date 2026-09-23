<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Trash2 } from '@lucide/svelte'
  import AddRow from '@/components/ui/AddRow.svelte'
  import ColorsMenu from '@/components/ui/ColorsMenu.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import RenameField from '@/components/ui/RenameField.svelte'
  import {
    useBoardLabels,
    createBoardLabel,
    updateBoardLabel,
    deleteBoardLabel,
    type Board,
  } from '@/lib/boards.svelte'
  import { listStagger } from '@/lib/motion'

  // ── Labels: the board's label registry — create, rename (cascades into
  //    tickets), recolor, delete (strips off tickets). ─────────────────────
  let { board }: { board: Board } = $props()

  const qc = useQueryClient()
  // A failed registry read used to render an empty label list under prose that
  // explains what labels do — read as "this board has none", which invites
  // creating duplicates of labels that already exist.
  const labelsQuery = useBoardLabels(() => board.id)
  const labels = $derived(labelsQuery.data ?? [])
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['board-labels', board.id] })
    void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  }
</script>

<div class="space-y-3">
  <p class="font-sans text-xs text-muted">
    Labels are shared by everyone on this board. Renaming updates every ticket carrying the label; deleting removes
    it from tickets.
  </p>
  {#if labelsQuery.isError}
    <QueryError
      variant={labelsQuery.data === undefined ? 'compact' : 'inline'}
      title={labelsQuery.data === undefined ? 'Could not load labels' : 'Labels may be out of date'}
      error={labelsQuery.error}
      onRetry={() => void labelsQuery.refetch()}
    />
  {/if}
  <ul class="divide-y divide-line-subtle" use:listStagger>
    {#each labels as l (l.id)}
      <li class="flex items-center gap-2 py-2">
        <ColorsMenu
          value={l.color}
          disabled={!canEdit}
          onPick={(c) => void updateBoardLabel(board.id, l.id, { color: c }).then(refresh)}
        />
        <RenameField
          value={l.name}
          key={`${l.id}-${l.name}`}
          disabled={!canEdit}
          class="flex-1"
          onCommit={(v) => void updateBoardLabel(board.id, l.id, { name: v }).then(refresh)}
        />
        {#if canEdit}
          <button
            title="Delete label (removes it from tickets)"
            onclick={() =>
              void (async () => {
                if (await confirm({ title: `Delete label "${l.name}"?`, message: 'It is removed from every ticket carrying it.', danger: true })) {
                  await deleteBoardLabel(board.id, l.id)
                  refresh()
                }
              })()}
            class="shrink-0 text-muted transition-colors hover:text-danger"
          >
            <Trash2 size={14} />
          </button>
        {/if}
      </li>
    {/each}
    <!-- Only once the server actually SAID so — otherwise the error above
         and "No labels yet." sit on screen contradicting each other. -->
    {#if labels.length === 0 && labelsQuery.data !== undefined}
      <li class="py-3 font-sans text-xs text-muted">No labels yet.</li>
    {/if}
  </ul>
  <!-- Nothing to add against: the registry never loaded, so "Add" here is a
       coin flip on whether the label already exists. -->
  {#if canEdit && labelsQuery.data !== undefined}
    <AddRow
      placeholder="New label"
      onSubmit={(name) => createBoardLabel(board.id, name).then(() => refresh())}
    />
  {/if}
</div>
