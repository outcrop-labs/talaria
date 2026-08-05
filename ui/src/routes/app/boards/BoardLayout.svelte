<script lang="ts">
  import type { Snippet } from 'svelte'
  import { preload } from '@/router'
  import Board from './Board.svelte'

  // Persistent board shell (sv-router layout): navigating between
  // /boards/:boardId and /boards/:boardId/:taskId swaps only the child —
  // the board itself stays MOUNTED. Before this, the ticket route rendered
  // its own <Board/>, so opening a ticket re-rendered (and re-staggered)
  // the entire board behind the modal — the jank this file exists to kill.
  let { children }: { children: Snippet } = $props()

  // Warm the ticket route's chunk (TaskDetail + the tiptap editor stack — the
  // heaviest module graph in the app) as soon as the board is idle. Cold, its
  // first-time evaluation blocks the main thread right as the modal mounts,
  // and the entrance has already elapsed by first paint: "the first ticket
  // you open doesn't animate". Card hover preloads too; this covers
  // keyboard/touch paths that never hover.
  $effect(() => {
    const idle = requestIdleCallback ?? ((fn: () => void) => setTimeout(fn, 250))
    const id = idle(() => void preload('/boards/:boardId/:taskId'))
    return () => (cancelIdleCallback ?? clearTimeout)(id as number)
  })
</script>

<Board />
{@render children()}
