<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ArrowRight, TriangleAlert } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Select from '@/components/ui/Select.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { moveTask, useBoardMembers, useBoards } from '@/lib/boards.svelte'
  import { useBoardStatuses } from '@/lib/statuses'
  import { moveChangeLines, moveDestinations, moveLosesSomething, type MoveSummary } from '@/lib/ticket-move'
  import type { Task } from '@/lib/task-const'

  // "Move to board…" — the destination picker, the column picker, and a
  // confirm that says what changes BEFORE anything does. The server answers
  // the "what changes" (POST …/move with preview: true) from the same code
  // that performs the move, so the dialog never guesses what will be dropped.
  let {
    task,
    boardId,
    open,
    onClose,
    onMoved,
  }: {
    task: Task
    /** The board the ticket is on now. */
    boardId: string
    open: boolean
    onClose: () => void
    /** After a move lands: the ticket on its new board. */
    onMoved?: (task: Task, moved: MoveSummary) => void
  } = $props()

  const qc = useQueryClient()
  const boardsList = listQuery(useBoards(), { title: 'Could not load your boards', variant: 'inline' })
  const destinations = $derived(moveDestinations(boardsList.rows, boardId))
  let destId = $state('')
  // '' = let the server map the column (same name → same kind → intake).
  let statusKey = $state('')
  const destStatuses = useBoardStatuses(() => destId || null)
  const srcStatuses = useBoardStatuses(() => boardId)
  const membersQuery = useBoardMembers(() => boardId)
  // A closed ticket may land in a done column (it is already closed); an
  // open one never does — a move is not a sign-off, and the server refuses it.
  const closed = $derived(
    (srcStatuses.data ?? []).some((s) => s.key === task.status && s.category === 'done') ||
      task.status === 'failed' ||
      task.status === 'cancelled',
  )
  const columnChoices = $derived((destStatuses.data ?? []).filter((s) => closed || s.category !== 'done'))
  const destName = $derived(destinations.find((b) => b.id === destId)?.name ?? '')
  const nameOf = (id: string) => {
    if (!id.startsWith('user:')) return id
    const m = (membersQuery.data ?? []).find((x) => `user:${x.userId}` === id)
    return m?.name ?? m?.email ?? id
  }

  let preview = $state<MoveSummary | null>(null)
  let previewError = $state<string | null>(null)
  let previewing = $state(false)
  let moving = $state(false)
  let moveError = $state<string | null>(null)

  // Reset on open, and pick the column afresh whenever the board changes —
  // the old board's key means nothing on the new one.
  $effect(() => {
    if (!open) return
    destId = ''
    statusKey = ''
    preview = null
    previewError = null
    moveError = null
  })
  let lastDest = ''
  $effect(() => {
    if (destId !== lastDest) {
      lastDest = destId
      statusKey = ''
    }
  })

  // The preview: re-asked on every choice, and only the newest answer counts.
  let seq = 0
  $effect(() => {
    const boardId = destId
    const status = statusKey
    if (!open || !boardId) {
      preview = null
      previewError = null
      return
    }
    const mine = ++seq
    previewing = true
    previewError = null
    moveTask(task.id, { boardId, status: status || undefined, preview: true })
      .then((r) => {
        if (mine === seq) preview = r.moved
      })
      .catch((e: unknown) => {
        if (mine !== seq) return
        preview = null
        previewError = e instanceof Error ? e.message : 'Could not preview the move.'
      })
      .finally(() => {
        if (mine === seq) previewing = false
      })
  })

  const lines = $derived(preview ? moveChangeLines(preview, nameOf) : [])
  const warns = $derived(!!preview && moveLosesSomething(preview))

  async function confirmMove() {
    if (!destId || !preview) return
    moving = true
    moveError = null
    try {
      const r = await moveTask(task.id, { boardId: destId, status: statusKey || undefined })
      void qc.invalidateQueries({ queryKey: ['board-tasks', boardId] })
      void qc.invalidateQueries({ queryKey: ['board-tasks', destId] })
      void qc.invalidateQueries({ queryKey: ['task', task.id] })
      if (r.task) onMoved?.(r.task, r.moved)
      onClose()
    } catch (e) {
      moveError = e instanceof Error ? e.message : 'The move did not go through.'
    } finally {
      moving = false
    }
  }
</script>

<Modal {open} {onClose} title="Move to another board" width="max-w-lg">
  <div class="space-y-4 font-sans text-sm">
    <p class="text-muted">
      {task.ticketRef ? `${task.ticketRef} · ` : ''}{task.title}
    </p>

    {#if boardsList.failed && boardsList.notice}
      <QueryError {...boardsList.notice} />
    {:else if !boardsList.pending && destinations.length === 0}
      <p class="text-muted">There is no other board you can add tickets to. You need to be an editor or owner of the board a ticket moves to.</p>
    {:else}
      <label class="block space-y-1">
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">Board</span>
        <Select bind:value={destId} class="w-full" aria-label="Destination board">
          <option value="" disabled>Choose a board…</option>
          {#each destinations as b (b.id)}
            <option value={b.id}>{b.name}{b.teamName ? ` — ${b.teamName}` : ''}</option>
          {/each}
        </Select>
      </label>

      {#if destId}
        <label class="block space-y-1">
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">Column</span>
          <Select bind:value={statusKey} class="w-full" aria-label="Destination column" disabled={destStatuses.isPending}>
            <option value="">Match it for me</option>
            {#each columnChoices as s (s.key)}
              <option value={s.key}>{s.label}</option>
            {/each}
          </Select>
        </label>
      {/if}
    {/if}

    {#if previewError}
      <p class="rounded-md border border-danger/50 bg-danger/5 p-2 text-danger">{previewError}</p>
    {:else if preview}
      <div
        class="space-y-1.5 rounded-lg border p-3 {warns ? 'border-warning/50 bg-warning/5' : 'border-line-subtle bg-raised'}"
        aria-live="polite"
      >
        <div class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
          {#if warns}<TriangleAlert size={12} />{/if}
          What changes
        </div>
        <ul class="space-y-1 text-fg">
          {#each lines as line (line)}<li>{line}</li>{/each}
        </ul>
      </div>
    {:else if previewing}
      <p class="text-muted">Checking what changes…</p>
    {/if}

    {#if moveError}
      <p class="rounded-md border border-danger/50 bg-danger/5 p-2 text-danger">{moveError}</p>
    {/if}
  </div>

  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onclick={onClose}>Cancel</Button>
      <Button size="sm" class="gap-1.5" disabled={!preview || previewing || moving} onclick={() => void confirmMove()}>
        <ArrowRight size={14} />
        {moving ? 'Moving…' : destName ? `Move to ${destName}` : 'Move'}
      </Button>
    </div>
  {/snippet}
</Modal>
