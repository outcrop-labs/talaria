<script lang="ts">
  import { searchParams } from 'sv-router'
  import { navigate, route } from '@/router'
  import TaskDetail from '@/components/board/TaskDetail.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { firstNotice, listQuery } from '@/components/ui/query-state'
  import { useBoards, useArchivedBoards, useTask } from '@/lib/boards.svelte'

  // Directly-linkable ticket route — the OVERLAY only. BoardLayout (the
  // persistent sv-router layout above this route) draws the board, so this
  // component must never mount its own <Board/>: that was a full board
  // re-render behind the modal on every ticket open.
  const params = $derived(route.getParams('/boards/:boardId/:taskId'))
  const boardId = $derived(params.boardId)
  const taskId = $derived(params.taskId)
  // This route is what a pasted ticket link lands on. Both lists defaulted to
  // `[]`, so `board` came out undefined and the whole modal returned null: a
  // link that opens a board with no ticket on it and no word about why. The
  // rows behave the same; the failure now has somewhere to go.
  const boardsList = listQuery(useBoards(), { title: 'Could not open this ticket', variant: 'full' })
  const archivedList = listQuery(useArchivedBoards(), { title: 'Could not open this ticket', variant: 'full' })
  const board = $derived(
    boardsList.rows.find((b) => b.id === boardId) ?? archivedList.rows.find((b) => b.id === boardId),
  )

  // A LINK THAT OUTLIVED A MOVE, for someone who cannot see the OLD board.
  // The ticket id never changes, so /boards/<old>/<ticket> still names it;
  // but this route looks the board up in the viewer's own lists, and a
  // person who is only on the NEW board finds no board here, so TaskDetail
  // (whose redirect covers everyone else) never mounts. Ask the ticket where
  // it lives — its read is gated on the board it is on NOW — and follow it.
  // Only once both lists answered and neither failed: a failure has its own
  // notice below, and a still-loading list may yet hold the board.
  const settled = $derived(![boardsList.pending, archivedList.pending, boardsList.failed, archivedList.failed].includes(true))
  const lookup = useTask(() => (!board && settled ? taskId : null))
  $effect(() => {
    const at = lookup.data?.task?.boardId
    if (at && at !== boardId) void navigate('/boards/:boardId/:taskId', { params: { boardId: at, taskId }, replace: true })
  })

  // Closing keeps the board's current view/filter state (the search travels).
  const close = () => void navigate('/boards/:boardId', { params: { boardId }, search: searchParams.toString() })
</script>

<!-- Nothing to look the ticket up IN. EITHER read failing is enough: the ticket
     may live on an archived board, so a live-boards list that came back fine
     does not license the conclusion "no such board" when the archived read
     broke. `&&` here meant a failed archived read fell through to the silent
     render-nothing below — the same blank modal this branch exists to replace.
     The board page behind this modal reports the failure for itself; this says
     why the ticket did not open. Both lists' flags are read before they are
     combined (query-state.ts, anyLoading). -->
{#if !board && [boardsList.failed, archivedList.failed].includes(true)}
  {@const notice = firstNotice(boardsList.notice, archivedList.notice)}
  <Modal open onClose={close} width="max-w-sm">
    {#if notice}<QueryError {...notice} />{/if}
  </Modal>
{:else if board}
  <TaskDetail {taskId} {board} onClose={close} />
{/if}
<!-- No board and no failure: still loading (or no access); the board page
     behind handles the empty state. -->
