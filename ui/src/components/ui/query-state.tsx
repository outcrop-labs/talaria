import type { ReactNode } from 'react'
import type { UseQueryResult } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { errorMessage } from '@/lib/fetch-json'

// The three honest answers a fetch can give: still loading, it BROKE, or it
// resolved (possibly to nothing). Surfaces used to collapse the first two into
// the third and render "No boards yet" over a 500 — this wrapper makes the
// broken case impossible to skip, because you have to pass a skeleton and you
// get the error branch for free.
//
// Loading stays the existing `Skeleton` shapes: pass the surface's own
// skeleton markup, don't let this component invent one (per UI-CONVENTIONS,
// the placeholder must match the content it stands in for).

/** The error branch on its own — for surfaces that fan several queries into one
 *  view and need to render the failure at their own seam. */
export function QueryError({
  error,
  title = 'Could not load this',
  onRetry,
  variant = 'full',
  className,
}: {
  error: unknown
  title?: string
  onRetry?: () => void
  variant?: 'full' | 'compact' | 'inline'
  className?: string
}) {
  const hint = errorMessage(error)
  if (variant === 'inline') {
    return (
      <div className={className}>
        <div className="text-xs text-[color:var(--theme-danger)]">{title}</div>
        <div className="mt-0.5 text-xs text-muted">{hint}</div>
        {onRetry && (
          <Button variant="link" size="sm" className="mt-1" onClick={onRetry}>
            Retry
          </Button>
        )}
      </div>
    )
  }
  return (
    <EmptyState
      variant={variant}
      className={className}
      icon={<span className="text-[color:var(--theme-danger)]">⚠</span>}
      title={title}
      hint={hint}
      action={
        onRetry ? (
          <Button variant="outline" size="sm" onClick={onRetry}>
            Retry
          </Button>
        ) : undefined
      }
    />
  )
}

/** What a list read is doing right now, with the failure ALREADY RENDERED. */
export interface ListQuery<T> {
  /** The rows. `[]` here means the server said `[]`, or the read has not
   *  happened yet — check `pending`/`failed` before calling it empty. */
  rows: T[]
  /** Render this. `null` when the read is fine; otherwise it is the failure,
   *  already shaped and already wired to Retry. It is the entire reason this
   *  function exists: you cannot obtain `rows` without also being handed the
   *  sentence that says they are missing. */
  notice: ReactNode
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
 */
export function listQuery<T>(
  // Same `Pick` as QueryState, and for the same reason: `isPending` +
  // `fetchStatus` are the only way to tell a DISABLED query (`enabled: !!id`)
  // from a slow one.
  query: Pick<UseQueryResult<T[]>, 'data' | 'isError' | 'error' | 'refetch' | 'isPending' | 'fetchStatus'>,
  opts: {
    /** What could not be loaded, in the surface's own words. */
    title: string
    /** Shown when stale rows are still up. Defaults to a "may be out of date" line. */
    staleTitle?: string
    /** Density of the replacement notice (`failed`). The stale marker is always inline. */
    variant?: 'full' | 'compact' | 'inline'
  },
): ListQuery<T> {
  const { data, isError, error, refetch, isPending, fetchStatus } = query
  const rows = data ?? []
  const failed = isError && data === undefined
  const stale = isError && data !== undefined
  const retry = () => void refetch()
  return {
    rows,
    failed,
    stale,
    pending: data === undefined && isPending && fetchStatus !== 'idle',
    notice: failed ? (
      <QueryError error={error} title={opts.title} variant={opts.variant ?? 'compact'} onRetry={retry} />
    ) : stale ? (
      <QueryError
        error={error}
        title={opts.staleTitle ?? 'This list may be out of date'}
        variant="inline"
        onRetry={retry}
        className="mb-2"
      />
    ) : null,
  }
}

const isEmptyValue = (d: unknown): boolean => (Array.isArray(d) ? d.length === 0 : d === null)

export function QueryState<T>({
  query,
  skeleton,
  empty,
  idle,
  isEmpty = isEmptyValue,
  errorTitle,
  errorVariant = 'full',
  children,
}: {
  // `isPending` + `fetchStatus` are part of the contract, not extras: without
  // them a DISABLED query (`enabled: !!boardId`, the shape half of lib/** uses)
  // is indistinguishable from a slow one — data undefined, isError false, for
  // ever — and this component would shimmer a skeleton at a request that is
  // never going to be made. Pass the whole `useQuery` result.
  query: Pick<UseQueryResult<T>, 'data' | 'isError' | 'error' | 'refetch' | 'isPending' | 'fetchStatus'>
  /** The surface's own Skeleton markup, shaped like the coming content. */
  skeleton: ReactNode
  /** Rendered only once the query RESOLVED empty — never while loading. */
  empty?: ReactNode
  /** Rendered while the query is DISABLED and has never run (no id selected
   *  yet). Defaults to a neutral "nothing selected" line rather than a blank
   *  region — a wordless gap reads as a broken view. Pass your own whenever the
   *  query is switched off for a reason a reader could not guess (a permission
   *  they don't have, a feature that isn't on): say THAT instead. */
  idle?: ReactNode
  /** Defaults to "empty array or null". Override for wrapper-shaped payloads. */
  isEmpty?: (data: T) => boolean
  errorTitle?: string
  /** Density of the non-content branches (error, and the default idle line):
   *  'full' centres in the region, 'compact' sits inside a panel, 'inline' is
   *  one quiet line. */
  errorVariant?: 'full' | 'compact' | 'inline'
  children: (data: T) => ReactNode
}) {
  const { data, isError, error, refetch, isPending, fetchStatus } = query
  // A failed BACKGROUND refetch keeps showing the last good data — stale beats
  // blank. Only a failure with nothing to fall back on takes over the surface.
  if (isError && data === undefined)
    return <QueryError error={error} title={errorTitle} variant={errorVariant} onRetry={() => void refetch()} />
  // Pending + idle = switched off. An enabled query is already 'fetching' on
  // its very first render, so this branch can only be the disabled case. The
  // default says so in words at the surface's own density — silence here is how
  // a disabled query gets mistaken for a view that failed to render.
  if (data === undefined && isPending && fetchStatus === 'idle')
    return (
      <>
        {idle === undefined ? (
          <EmptyState variant={errorVariant} title="Nothing selected" hint="Pick one from the list to see it here." />
        ) : (
          // An explicit `idle={null}` still means "deliberately nothing here".
          idle
        )}
      </>
    )
  if (data === undefined) return <>{skeleton}</>
  if (empty !== undefined && isEmpty(data)) return <>{empty}</>
  return <>{children(data)}</>
}
