<script lang="ts" module>
  import type { AppSurfaces } from '@/sdk'
  import { appLoader } from '@/lib/apps'

  // Renders one surface (work | manage | settings) of an enabled app, natively:
  // same router, same session, same design system — the app's chunk lazy-loads
  // on first visit and stays cached (module-level promise map survives remounts).
  type Surface = 'work' | 'manage' | 'settings'
  type SurfaceComponent = NonNullable<AppSurfaces[Surface]>

  const surfaceCache = new Map<string, Promise<SurfaceComponent | null>>()

  function surfaceComponent(slug: string, surface: Surface): Promise<SurfaceComponent | null> | null {
    const key = `${slug}:${surface}`
    const hit = surfaceCache.get(key)
    if (hit) return hit
    const loader = appLoader(slug)
    if (!loader) return null
    // Resolving `null` = the module loaded but provides no such surface — the
    // template renders the "Missing surface" empty state for it.
    const promise = loader().then((mod) => mod.default?.[surface] ?? null)
    // A rejected chunk fetch is evicted so a later visit retries it instead of
    // latching the rejection forever (React lazy cached rejections; its
    // ErrorBoundary resetKey papered over that — here the cache itself heals).
    promise.catch(() => surfaceCache.delete(key))
    surfaceCache.set(key, promise)
    return promise
  }
</script>

<script lang="ts">
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import ErrorFallback from '@/components/ui/ErrorFallback.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { useEnabledApps } from '@/lib/apps'

  let { slug, surface }: { slug: string; surface: Surface } = $props()

  const appsQuery = useEnabledApps()
  const app = $derived(appsQuery.data?.find((a) => a.slug === slug))
  // Bumped by the ErrorFallback's reset after a failed chunk load: the cache
  // entry was evicted on rejection, so re-deriving asks the network again.
  let retry = $state(0)
  const compPromise = $derived.by(() => {
    void retry
    return surfaceComponent(slug, surface)
  })
</script>

{#snippet surfaceSkeleton()}
  <div class="p-8">
    <div class="mx-auto w-full max-w-[var(--page-width)] space-y-6">
      <Skeleton class="h-6 w-56 rounded-full" />
      <div class="rounded-lg border border-line bg-panel p-6">
        <SkeletonRows rows={6} />
      </div>
    </div>
  </div>
{/snippet}

<!-- Three states, not two. `isLoading` alone leaves the failed read looking
     identical to a resolved one: `apps` stays undefined, the find below misses,
     and the page tells a reader the app is "disabled, not installed, or awaiting
     a rebuild" when the truth is /api/apps answered 500 — an accusation that
     sends someone off to reinstall something that was never broken. -->
{#if appsQuery.isError && appsQuery.data === undefined}
  <QueryError
    error={appsQuery.error}
    title="Could not load your apps"
    onRetry={() => void appsQuery.refetch()}
  />
{:else if appsQuery.data === undefined}
  <!-- Undefined with no error is still in flight (first load or a retry after a
       cleared cache) — hold the skeleton rather than judging the app missing. -->
  {@render surfaceSkeleton()}
{:else if !app || !app.surfaces[surface]}
  <EmptyState title="App not available" hint="It may be disabled, not installed, or awaiting a rebuild. Check Manage → Apps" />
{:else if !compPromise}
  <EmptyState title="App code not in this build" hint="The app is installed but needs a dev-server reload or a rebuild to load" />
{:else}
  <!-- The lazy import can reject — a deploy rotated the chunk hash out from under
       an open tab, the app's bundle throws at module scope, or the network drops
       mid-fetch. Without a boundary the whole tree would come down and the cockpit
       goes white. The key is the surface identity, so navigating elsewhere and
       back gives the chunk another chance instead of latching the error forever. -->
  {#key `${slug}:${surface}`}
    <svelte:boundary>
      {#await compPromise}
        {@render surfaceSkeleton()}
      {:then C}
        {#if C}
          <C />
        {:else}
          <EmptyState title="Missing surface" hint={`${slug} does not provide a ${surface} surface`} />
        {/if}
      {:catch error}
        <ErrorFallback {error} what={app.name} reset={() => (retry += 1)} />
      {/await}
      {#snippet failed(error, reset)}
        <ErrorFallback {error} {reset} what={app.name} />
      {/snippet}
    </svelte:boundary>
  {/key}
{/if}
