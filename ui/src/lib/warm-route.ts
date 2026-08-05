import { preload } from '@/router'

// The one warm-up wrapper for any element that opens an INLINE route — a
// child route that mounts inside the current view (the ticket modal being
// the archetype). Cold, a lazy route chunk's first evaluation blocks the
// main thread right as the overlay mounts, and its entrance animation has
// elapsed before first paint ("the first ticket you open doesn't animate");
// a cold query adds skeleton-then-pop on top. Warming both on intent
// (hover/focus/touch) makes the overlay open whole and animated.
//
//   <tr use:warmRoute={{ path: '/boards/:boardId/:taskId', warm: () => prefetchTask(qc, t.id) }}>
//
// An action, not a wrapper component, on purpose: it attaches to the
// caller's own element (a <tr>, a card div, a button) without touching
// layout. Fires on every intent signal — both legs must stay idempotent
// (preload caches modules; query prefetches dedupe via staleTime).
interface WarmRouteOptions {
  /** Route pattern whose lazy chunks to warm — typed against the route tree. */
  path?: Parameters<typeof preload>[0]
  /** Data warm-up to run alongside (query prefetch etc.). */
  warm?: () => void
}

export function warmRoute(node: HTMLElement, opts: WarmRouteOptions) {
  const fire = () => {
    if (opts.path) void preload(opts.path)
    opts.warm?.()
  }
  node.addEventListener('pointerenter', fire)
  node.addEventListener('focusin', fire)
  node.addEventListener('touchstart', fire, { passive: true })
  return {
    destroy() {
      node.removeEventListener('pointerenter', fire)
      node.removeEventListener('focusin', fire)
      node.removeEventListener('touchstart', fire)
    },
  }
}
