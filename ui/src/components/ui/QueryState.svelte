<script lang="ts" generics="T">
  import type { Snippet } from 'svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from './QueryError.svelte'
  import { isEmptyValue, type QueryLike } from './query-state'

  // See query-state.ts for the contract: still loading, it BROKE, or it
  // resolved (possibly to nothing) — and the broken case cannot be skipped.
  let {
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
    // never going to be made. Pass the whole `createQuery` result (never
    // destructured — reads below go through `query.*` to stay reactive).
    query: QueryLike<T>
    /** The surface's own Skeleton markup, shaped like the coming content. */
    skeleton: Snippet
    /** Rendered only once the query RESOLVED empty — never while loading. */
    empty?: Snippet
    /** Rendered while the query is DISABLED and has never run (no id selected
     *  yet). Defaults to a neutral "nothing selected" line rather than a blank
     *  region — a wordless gap reads as a broken view. Pass your own whenever the
     *  query is switched off for a reason a reader could not guess (a permission
     *  they don't have, a feature that isn't on): say THAT instead. */
    idle?: Snippet | null
    /** Defaults to "empty array or null". Override for wrapper-shaped payloads. */
    isEmpty?: (data: T) => boolean
    errorTitle?: string
    /** Density of the non-content branches (error, and the default idle line):
     *  'full' centres in the region, 'compact' sits inside a panel, 'inline' is
     *  one quiet line. */
    errorVariant?: 'full' | 'compact' | 'inline'
    children: Snippet<[T]>
  } = $props()
</script>

<!-- A failed BACKGROUND refetch keeps showing the last good data — stale beats
     blank. Only a failure with nothing to fall back on takes over the surface. -->
{#if query.isError && query.data === undefined}
  <QueryError error={query.error} title={errorTitle} variant={errorVariant} onRetry={() => void query.refetch()} />
{:else if query.data === undefined && query.isPending && query.fetchStatus === 'idle'}
  <!-- Pending + idle = switched off. An enabled query is already 'fetching' on
       its very first render, so this branch can only be the disabled case. The
       default says so in words at the surface's own density — silence here is how
       a disabled query gets mistaken for a view that failed to render. -->
  {#if idle === undefined}
    <EmptyState variant={errorVariant} title="Nothing selected" hint="Pick one from the list to see it here." />
  {:else if idle !== null}
    <!-- An explicit `idle={null}` still means "deliberately nothing here". -->
    {@render idle()}
  {/if}
{:else if query.data === undefined}
  {@render skeleton()}
{:else if empty !== undefined && isEmpty(query.data)}
  {@render empty()}
{:else}
  {@render children(query.data)}
{/if}
