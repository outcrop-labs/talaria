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
    // `typeof`, never a bare reference: WebKit (Safari — and WebKitGTK, the
    // desktop shell's engine) never shipped requestIdleCallback, and reading
    // an absent global by name throws ReferenceError before a `??` fallback
    // can run. "Can't find variable: requestIdleCallback" was exactly that.
    // Swallow, don't surface twice: a warm-up that fails lands in the router's
    // onError hook (reported to the recovery banner); the catch only keeps
    // this fire-and-forget from ALSO raising an unhandled rejection.
    if (typeof requestIdleCallback !== 'function') {
      const t = setTimeout(() => void preload('/boards/:boardId/:taskId').catch(() => {}), 250)
      return () => clearTimeout(t)
    }
    const id = requestIdleCallback(() => void preload('/boards/:boardId/:taskId').catch(() => {}))
    return () => cancelIdleCallback(id)
  })
</script>

<Board />
{@render children()}
