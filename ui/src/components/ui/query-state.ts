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

/** Default emptiness test for QueryState: "empty array or null". */
export const isEmptyValue = (d: unknown): boolean => (Array.isArray(d) ? d.length === 0 : d === null)
