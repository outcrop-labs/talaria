<script lang="ts">
  import type { Snippet } from 'svelte'
  import Board from './Board.svelte'

  // Persistent board shell (sv-router layout): navigating between
  // /boards/:boardId and /boards/:boardId/:taskId swaps only the child —
  // the board itself stays MOUNTED. Before this, the ticket route rendered
  // its own <Board/>, so opening a ticket re-rendered (and re-staggered)
  // the entire board behind the modal — the jank this file exists to kill.
  let { children }: { children: Snippet } = $props()
</script>

<Board />
{@render children()}
