<script lang="ts" module>
  import type { AppSurfaces } from '@/sdk'
  import { appLoader } from '@/lib/apps'

  // Renders one surface (work | manage | settings) of an enabled app, natively:
  // same router, same session, same design system — the app's chunk lazy-loads
  // on first visit and stays cached (module-level promise map survives remounts).
  type Surface = 'work' | 'manage' | 'settings'
  type SurfaceComponent = NonNullable<AppSurfaces[Surface]>

  const surfaceCache = new Map<string, Promise<SurfaceComponent | null>>()

  function surfaceComponent(
    slug: string,
    surface: Surface,
    buildKey?: string | null,
  ): Promise<SurfaceComponent | null> | null {
    const key = `${slug}:${surface}:${buildKey ?? 'dev'}`
    const hit = surfaceCache.get(key)
    if (hit) return hit
    const loader = appLoader(slug, buildKey)
    if (!loader) return null
    const promise = loader().then((mod) => mod.default?.[surface] ?? null)
    promise.catch(() => surfaceCache.delete(key))
    surfaceCache.set(key, promise)
    return promise
  }
</script>

<script lang="ts">
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import AppCrash from './AppCrash.svelte'

  let { slug, surface }: { slug: string; surface: Surface } = $props()

  const appsQuery = useEnabledApps()
  const app = $derived(appsQuery.data?.find((a) => a.slug === slug))
  let retry = $state(0)
  const compPromise = $derived.by(() => {
    void retry
    return surfaceComponent(slug, surface, app?.build?.key)
  })
</script>

<svelte:head>
  {#if !import.meta.env.DEV && app?.build?.status === 'ready' && app.build.key}
    <link rel="stylesheet" href={`/app-builds/${slug}/${app.build.key}/app.css`} />
  {/if}
</svelte:head>

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

{#if appsQuery.isError && appsQuery.data === undefined}
  <QueryError
    error={appsQuery.error}
    title="Could not load your apps"
    onRetry={() => void appsQuery.refetch()}
  />
{:else if appsQuery.data === undefined}
  {@render surfaceSkeleton()}
{:else if !app || !app.surfaces[surface]}
  <EmptyState title="App not available" hint="It may be disabled, not installed, or still compiling. Check Manage → Apps" />
{:else if app.build?.status === 'failed'}
  <AppCrash name={app.name} error={app.build.error ?? 'build failed'} compiling reset={() => (retry += 1)} />
{:else if app.build?.status === 'building'}
  <EmptyState title="Compiling {app.name}" hint="The instance is compiling this app. It will appear here when the build finishes." />
{:else if !compPromise}
  <EmptyState title="Compiling {app.name}" hint="The app is installed and will appear here once this instance finishes compiling it." />
{:else}
  {#key `${slug}:${surface}:${retry}`}
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
        <AppCrash {error} name={app.name} reset={() => (retry += 1)} />
      {/await}
      {#snippet failed(error, reset)}
        <AppCrash {error} name={app.name} {reset} />
      {/snippet}
    </svelte:boundary>
  {/key}
{/if}
