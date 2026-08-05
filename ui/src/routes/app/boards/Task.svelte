<script lang="ts">
  import { searchParams } from 'sv-router'
  import { navigate, route } from '@/router'
  import TaskDetail from '@/components/board/TaskDetail.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useBoards, useArchivedBoards } from '@/lib/boards.svelte'

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

  // Closing keeps the board's current view/filter state (the search travels).
  const close = () => void navigate('/boards/:boardId', { params: { boardId }, search: searchParams.toString() })
</script>

<!-- Nothing to look the ticket up IN. EITHER read failing is enough: the ticket
     may live on an archived board, so a live-boards list that came back fine
     does not license the conclusion "no such board" when the archived read
     broke. `&&` here meant a failed archived read fell through to the silent
     render-nothing below — the same blank modal this branch exists to replace.
     The board page behind this modal reports the failure for itself; this says
     why the ticket did not open. -->
{#if !board && (boardsList.failed || archivedList.failed)}
  {@const notice = boardsList.notice ?? archivedList.notice}
  <Modal open onClose={close} width="max-w-sm">
    {#if notice}<QueryError {...notice} />{/if}
  </Modal>
{:else if board}
  <TaskDetail {taskId} {board} onClose={close} />
{/if}
<!-- No board and no failure: still loading (or no access); the board page
     behind handles the empty state. -->
