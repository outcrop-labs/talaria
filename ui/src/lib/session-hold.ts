// The session gate's hold timer.
//
// AppLayout paints its auth hold as a brand mark on Mercury ground and
// NOTHING else — deliberately: the wait is one round-trip, and chrome that
// flashes in before the session answers would read as a lie (see the comment
// on the gate in AppLayout.svelte). But a wedged session read — the dropped
// keep-alive shape a container roll leaves behind; the read deadline in
// fetch-json.ts documents the family — holds that frame for the read's 30s
// abort plus the query client's one retry: the better part of a minute of a
// screen with zero interactive elements, which is GH #327's "requires a
// refresh to interact" in its residual form. The hold is honest for one
// round-trip; past that it owes the person a way out, and this is the timing
// half of one.
//
// Plain TypeScript on purpose, not a .svelte.ts runes module: the ui vitest
// config runs a node environment with no Svelte compiler, so the reactivity
// stays in the component's $effect and what is left here is a testable
// arm/disarm pair.
//
//   let holdTimedOut = $state(false)
//   $effect(() => {
//     const pending = query.isLoading || !data
//     if (!pending) { holdTimedOut = false; return }
//     return startHoldGrace(() => pending, () => (holdTimedOut = true))
//   })
//
// The caller's $effect IS the reactivity: it reads the pending condition,
// re-runs when it flips, and runs the previous stop() first — which is what
// disarms an in-flight grace the moment the condition clears. A new pending
// period is a new call, so the grace re-arms from zero every time.

/** One round-trip plus a breath. Past this the hold stops reading as
 *  "connecting" and starts reading as "stuck" — short enough to arrive while
 *  the person is still looking, long enough that no honest session read ever
 *  trips it. */
export const SESSION_HOLD_GRACE_MS = 12_000

/**
 * Arm the grace for ONE pending period: call `onGrace` once, `graceMs` after
 * the call, unless the returned stop runs first. Reads `pending()` exactly
 * once, at arm time — pass a snapshot of the condition, not the reactive
 * getter, so this file stays runes-free.
 */
export function startHoldGrace(
  pending: () => boolean,
  onGrace: () => void,
  graceMs: number = SESSION_HOLD_GRACE_MS,
): () => void {
  // Not pending: nothing to arm. The empty stop keeps the caller's cleanup
  // contract uniform — an effect may return it unconditionally.
  if (!pending()) return () => {}
  const timer = setTimeout(onGrace, graceMs)
  return () => clearTimeout(timer)
}
