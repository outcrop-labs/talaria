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
