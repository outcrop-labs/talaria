// THE MERGED WORK BADGE'S NUMBER — work sessions plus plans plus research
// threads, as one count.
//
// /api/unreads answers an arm per surface that counts (comms, work, plan,
// research, notifications), and those arms stay per-kind because a pill, a
// surface and a future rail can each want one of them alone. What the rail's
// single Work entry needs is their total: an entry that counted only `work`
// would read "nothing waiting" while an unread plan turn sat in the surface
// that entry opens — which is the exact wrong answer on the one surface a
// person checks instead of opening anything.
//
// Pure, and its own file, so the sum is a thing with a name and a test rather
// than an expression in a query function where it cannot be reached.

/** The arms of the unreads answer this sum reads. Optional, not because the
 *  route omits them — it answers all three or 500s — but because a response
 *  from an api one deploy behind may not carry an arm this UI knows about, and
 *  `work + plan + research` with one of them `undefined` is `NaN`: a badge that
 *  renders nothing legible and cannot be told from a real count. */
export interface SessionArms {
  work?: number | null
  plan?: number | null
  research?: number | null
}

/** One arm as a number: a count, or 0 for an arm that is absent or not a finite
 *  number. Counting a missing arm as 0 keeps the two that DID arrive honest;
 *  the alternative is poisoning the whole badge with one gap. */
function arm(value: number | null | undefined): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

/**
 * Work sessions, plans and research threads as the one count the rail's Work
 * entry carries.
 *
 * `comms` and `notifications` are deliberately NOT summed: Comms has its own
 * rail entry and the bell has its own surface, so adding them here would count
 * the same unread twice in two places and make the Work badge a number nobody
 * could act on.
 *
 * There is no overload for a missing answer, and that is the doctrine rather
 * than an omission: the route 500s when any arm fails, the query lands in
 * error, and the badge renders `!` (see nav-badges.svelte.ts). A
 * `sumSessions(undefined) === 0` would invite a caller to paper a failed read
 * over with a confident zero — this app's oldest bug shape, on the surface that
 * gates every human decision. Callers hand this function data that arrived.
 */
export function sumSessions(unreads: SessionArms): number {
  return arm(unreads.work) + arm(unreads.plan) + arm(unreads.research)
}
