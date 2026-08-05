// Client registry for Talaria apps: which apps are enabled (server truth) and
// how to load their compiled surface modules (build-time glob, code-split per
// app — an app's bundle only downloads when one of its surfaces renders).
import { createQuery } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'
import type { AppSurfaces } from '@/sdk'

export interface AppManifest {
  slug: string
  name: string
  icon: string
  description: string
  version: string
  surfaces: { work?: string; manage?: string; settings?: string }
  mcp?: boolean
}

export function useEnabledApps() {
  return createQuery(() => ({
    queryKey: ['apps'],
    queryFn: (): Promise<AppManifest[]> => getList<AppManifest>('/api/apps', 'apps'),
    staleTime: 60_000,
  }))
}

// Both patterns during the Svelte migration: app.tsx entries become app.ts
// (the surfaces object is pure TS importing .svelte components) in the apps
// conversion wave; only one of the two exists per app at any time.
const LOADERS = import.meta.glob(['../../../apps/*/app.ts', '../../../apps/*/app.tsx']) as Record<string, () => Promise<{ default: AppSurfaces }>>

export function appLoader(slug: string): (() => Promise<{ default: AppSurfaces }>) | null {
  const entry = Object.entries(LOADERS).find(([p]) => /apps\/([^/]+)\//.exec(p)?.[1] === slug)
  return entry?.[1] ?? null
}
