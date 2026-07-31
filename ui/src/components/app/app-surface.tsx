import { Suspense, lazy, type ComponentType, type LazyExoticComponent } from 'react'
import { EmptyState } from '@/components/ui/empty-state'
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
  const { data: apps, isLoading } = useEnabledApps()
  if (isLoading) return <SurfaceSkeleton />
  const app = apps?.find((a) => a.slug === slug)
  if (!app || !app.surfaces[surface]) {
    return <EmptyState title="App not available" hint="It may be disabled, not installed, or awaiting a rebuild — check Manage → Apps" />
  }
  const C = surfaceComponent(slug, surface)
  if (!C) {
    return <EmptyState title="App code not in this build" hint="The app is installed but needs a dev-server reload or a rebuild to load" />
  }
  return (
    <Suspense fallback={<SurfaceSkeleton />}>
      <C />
    </Suspense>
  )
}
