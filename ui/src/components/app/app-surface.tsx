import { Suspense, lazy, type ComponentType, type LazyExoticComponent } from 'react'
import { EmptyState } from '@/components/ui/empty-state'
import { ErrorBoundary } from '@/components/ui/error-boundary'
import { QueryError } from '@/components/ui/query-state'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { appLoader, useEnabledApps } from '@/lib/apps'

// Renders one surface (work | manage | settings) of an enabled app, natively:
// same router, same session, same design system — the app's chunk lazy-loads
// on first visit and stays cached (module-level lazy map survives re-renders).
const lazyCache = new Map<string, LazyExoticComponent<ComponentType>>()

function surfaceComponent(slug: string, surface: 'work' | 'manage' | 'settings'): LazyExoticComponent<ComponentType> | null {
  const key = `${slug}:${surface}`
  const hit = lazyCache.get(key)
  if (hit) return hit
  const loader = appLoader(slug)
  if (!loader) return null
  const comp = lazy(async () => {
    const mod = await loader()
    const C = mod.default?.[surface]
    return { default: C ?? (() => <EmptyState title="Missing surface" hint={`${slug} does not provide a ${surface} surface`} />) }
  })
  lazyCache.set(key, comp)
  return comp
}

function SurfaceSkeleton() {
  return (
    <div className="p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <Skeleton className="h-6 w-56 rounded-full" />
        <div className="rounded-lg border border-line bg-panel p-6">
          <SkeletonRows rows={6} />
        </div>
      </div>
    </div>
  )
}

export function AppSurface({ slug, surface }: { slug: string; surface: 'work' | 'manage' | 'settings' }) {
  const appsQuery = useEnabledApps()
  const { data: apps } = appsQuery
  // Three states, not two. `isLoading` alone leaves the failed read looking
  // identical to a resolved one: `apps` stays undefined, the find below misses,
  // and the page tells a reader the app is "disabled, not installed, or awaiting
  // a rebuild" when the truth is /api/apps answered 500 — an accusation that
  // sends someone off to reinstall something that was never broken.
  if (appsQuery.isError && apps === undefined)
    return (
      <QueryError
        error={appsQuery.error}
        title="Could not load your apps"
        onRetry={() => void appsQuery.refetch()}
      />
    )
  // Undefined with no error is still in flight (first load or a retry after a
  // cleared cache) — hold the skeleton rather than judging the app missing.
  if (apps === undefined) return <SurfaceSkeleton />
  const app = apps.find((a) => a.slug === slug)
  if (!app || !app.surfaces[surface]) {
    return <EmptyState title="App not available" hint="It may be disabled, not installed, or awaiting a rebuild — check Manage → Apps" />
  }
  const C = surfaceComponent(slug, surface)
  if (!C) {
    return <EmptyState title="App code not in this build" hint="The app is installed but needs a dev-server reload or a rebuild to load" />
  }
  // The lazy import can reject — a deploy rotated the chunk hash out from under
  // an open tab, the app's bundle throws at module scope, or the network drops
  // mid-fetch. Without a boundary React unmounts the whole tree and the cockpit
  // goes white. The key is the surface identity, so navigating elsewhere and
  // back gives the chunk another chance instead of latching the error forever.
  return (
    <ErrorBoundary what={app.name} resetKey={`${slug}:${surface}`}>
      <Suspense fallback={<SurfaceSkeleton />}>
        <C />
      </Suspense>
    </ErrorBoundary>
  )
}
