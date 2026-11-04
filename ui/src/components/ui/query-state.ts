// The three honest answers a fetch can give: still loading, it BROKE, or it
// resolved (possibly to nothing). Surfaces used to collapse the first two into
// the third and render "No boards yet" over a 500 — QueryState.svelte makes
// the broken case impossible to skip, because you have to pass a skeleton and
// you get the error branch for free.
//
// Loading stays the existing `Skeleton` shapes: pass the surface's own
// skeleton markup, don't let this component invent one (per UI-CONVENTIONS,
// the placeholder must match the content it stands in for).
//
// Components: QueryState.svelte + QueryError.svelte; the list-read helper and
// shared types live here.

/** The slice of a `createQuery` result these helpers read — same `Pick` the
 *  React version took, kept structural so svelte-query's result type name
 *  stays out of the contract. `isPending` + `fetchStatus` are the only way to
 *  tell a DISABLED query (`enabled: !!id`) from a slow one. */
export interface QueryLike<T> {
  readonly data: T | undefined
  readonly isError: boolean
  readonly error: unknown
  readonly refetch: () => unknown
  readonly isPending: boolean
  readonly fetchStatus: 'fetching' | 'paused' | 'idle'
}

/** Props for QueryError.svelte — also the shape `listQuery` hands back as
 *  `notice`, ready to spread: `<QueryError {...list.notice} />`. */
export interface QueryErrorProps {
  error: unknown
  title?: string
  onRetry?: () => void
  variant?: 'full' | 'compact' | 'inline'
  class?: string
}

/** What a list read is doing right now, with the failure ALREADY SHAPED. */
export interface ListQuery<T> {
  /** The rows. `[]` here means the server said `[]`, or the read has not
   *  happened yet — check `pending`/`failed` before calling it empty. */
  rows: T[]
  /** Render this (`<QueryError {...notice} />`). `null` when the read is fine;
   *  otherwise it is the failure, already shaped and already wired to Retry.
   *  It is the entire reason this function exists: you cannot obtain `rows`
   *  without also being handed the sentence that says they are missing. */
  notice: QueryErrorProps | null
  /** The read failed and there is NOTHING to show. Render `notice` INSTEAD of
   *  the list — an empty list next to an error reads as "and also there are
   *  none", which is a second lie on top of the first. */
  failed: boolean
  /** The read failed but stale rows are still on screen. Render `notice`
   *  ABOVE the list: stale beats blank, as long as it says it's stale. */
  stale: boolean
  /** First load, still in flight. Show your skeleton — NOT an empty state. */
  pending: boolean
}

/** THE way to read a list query.
 *
 *  `const { data: users = [] } = useUsers()` was the most-copied line in this
 *  app and every copy is the same lie: `[]` is what you get from a 200 with no
 *  rows AND from a 500, so the surface renders "nobody works here" over an
 *  outage. Thirty-nine call sites wrote it. Every one of them also *discarded
 *  the query object*, which is what made the error unreachable — not an
 *  oversight at each site, a shape that had no room for the error.
 *
 *  This is that shape with room for the error. It returns the rows AND the
 *  failure, so the failure cannot be left behind at the call site: the only
 *  way to skip it now is to bind `notice` and then not render it, which is
 *  more work than rendering it and reads as deliberate in review.
 *
 *  `title` is required on purpose. "Could not load this" is never as useful as
 *  "Could not load who has access", and the moment the compiler asks you for a
 *  sentence you have to decide what this list actually is.
 *
 *  A 200 carrying `[]` gets `failed: false, notice: null` and your own empty
 *  state, exactly as before. Empty and broken stay different answers.
 *
 *  Everything is exposed through getters reading the live query object, so the
 *  result stays reactive — don't destructure it, read `list.rows` etc.
 */
export function listQuery<T>(
  query: QueryLike<T[]>,
  opts: {
    /** What could not be loaded, in the surface's own words. */
    title: string
    /** Shown when stale rows are still up. Defaults to a "may be out of date" line. */
    staleTitle?: string
    /** Density of the replacement notice (`failed`). The stale marker is always inline. */
    variant?: 'full' | 'compact' | 'inline'
  },
): ListQuery<T> {
  const failed = () => query.isError && query.data === undefined
  const stale = () => query.isError && query.data !== undefined
  const retry = () => void query.refetch()
  return {
    get rows() {
      return query.data ?? []
    },
    get failed() {
      return failed()
    },
    get stale() {
      return stale()
    },
    get pending() {
      return query.data === undefined && query.isPending && query.fetchStatus !== 'idle'
    },
    get notice(): QueryErrorProps | null {
      return failed()
        ? { error: query.error, title: opts.title, variant: opts.variant ?? 'compact', onRetry: retry }
        : stale()
          ? {
              error: query.error,
              title: opts.staleTitle ?? 'This list may be out of date',
              variant: 'inline',
              onRetry: retry,
              class: 'mb-2',
            }
          : null
    },
  }
}

/** Is any of these queries still on its first load (`isLoading`)? The way to
 *  gate one surface on several reads.
 *
 *  It reads EVERY query's flag before it combines them, and that is the whole
 *  point. `a.isLoading || b.isLoading` stops at `a` while `a` loads, so `b`'s
 *  flag is never read. svelte-query tells a component about a change only
 *  when it touches a prop the component has read (TanStack's tracked
 *  results), and a prop first read later comes from the last result the
 *  component was sent. So when `b` answered first, and nothing changed in
 *  what had been read from `b`, the component was not told. When `a` answered
 *  and the `||` finally read `b.isLoading`, it got `true` from before `b`
 *  answered. Nothing else would change after that, so the surface stayed on
 *  its skeletons for good. That was the agent roster whenever
 *  /api/fleet/containers answered before /api/fleet/defs. (A query nothing
 *  has read from yet is told about every change; it bit where something else
 *  read `b`, as the roster's banner reads the containers' `isError`. The
 *  reactive proof is query-state.svelte.test.ts.) */
export function anyLoading(...queries: ReadonlyArray<{ readonly isLoading: boolean }>): boolean {
  const flags = queries.map((q) => q.isLoading)
  return flags.includes(true)
}

/** `anyLoading` for `isPending` (a disabled query is pending too): read
 *  every flag, then combine, for the same reason. */
export function anyPending(...queries: ReadonlyArray<{ readonly isPending: boolean }>): boolean {
  const flags = queries.map((q) => q.isPending)
  return flags.includes(true)
}

/** `anyLoading` for `isError`: did any of these reads fail? Every flag is read
 *  before they are combined. `a.isError || b.isError` leaves `b.isError`
 *  unread while `a` has failed, so when `a` recovers the `||` reads `b`'s
 *  flag from whatever result the component was last sent. `undefined` stands
 *  for a read the surface does not have (an optional prop). */
export function anyError(...queries: ReadonlyArray<{ readonly isError: boolean } | undefined>): boolean {
  const flags = queries.map((q) => q?.isError === true)
  return flags.includes(true)
}

/** Did any of these reads fail with nothing to show (`isError` and no
 *  `data`)? The "render the error instead of the surface" test for a
 *  surface gated on several reads, read whole for each query before any
 *  is judged (`anyLoading` says why). */
export function anyFailed(...queries: ReadonlyArray<{ readonly isError: boolean; readonly data: unknown }>): boolean {
  const failed = queries.map((q) => [q.isError, q.data === undefined] as const)
  return failed.some(([isError, none]) => isError && none)
}

/** The first of these reads that failed, or null — every flag read first.
 *  For a surface that shows ONE failure (its error and its retry). */
export function firstError<Qs extends ReadonlyArray<{ readonly isError: boolean }>>(...queries: Qs): Qs[number] | null {
  const flags = queries.map((q) => q.isError)
  const i = flags.indexOf(true)
  return i === -1 ? null : queries[i]!
}

/** The first of these notices (`listQuery(...).notice`), or null. The
 *  arguments are evaluated before the call, so every list's notice is read
 *  before one is picked: `a.notice || b.notice` and `a.notice ?? b.notice`
 *  leave `b` unread while `a` has one, the same stale read `anyLoading`
 *  describes. A `listQuery` result's `pending`, `failed`, `stale` and
 *  `notice` are its query's flags under other names, so they follow the same
 *  rule: read every list's flag first, as `[a.pending, b.pending].includes(true)`
 *  or `firstNotice(a.notice, b.notice)`. */
export function firstNotice(...notices: ReadonlyArray<QueryErrorProps | null>): QueryErrorProps | null {
  return notices.find((n) => n !== null) ?? null
}

/** Default emptiness test for QueryState: "empty array or null". */
export const isEmptyValue = (d: unknown): boolean => (Array.isArray(d) ? d.length === 0 : d === null)
