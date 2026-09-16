// Client registry for Talaria apps: which apps are enabled (server truth) and
// how to load their surface modules. Dev loads source through Vite (`/@app/`)
// so authors keep HMR. Prod loads the independently-built chunk at
// `/app-builds/<slug>/<key>/app.js` — the host bundle never contains app code.
import { createQuery } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'
import type { AppSurfaces } from '@/sdk'

export interface AppBuildInfo {
  status: 'ready' | 'building' | 'failed' | 'none'
  key?: string
  error?: string
}

export interface AppManifest {
  slug: string
  name: string
  icon: string
  description: string
  version: string
  surfaces: { work?: string; manage?: string; settings?: string }
  mcp?: boolean
  build?: AppBuildInfo
}

export function useEnabledApps() {
  return createQuery(() => ({
    queryKey: ['apps'],
    queryFn: (): Promise<AppManifest[]> => getList<AppManifest>('/api/apps', 'apps'),
    staleTime: 60_000,
  }))
}

export function appLoader(slug: string, key?: string | null): (() => Promise<{ default: AppSurfaces }>) | null {
  if (import.meta.env.DEV) {
    return () => import(/* @vite-ignore */ `/@app/${slug}/app.ts`)
  }
  if (!key) return null
  return () => import(/* @vite-ignore */ `/app-builds/${slug}/${key}/app.js`)
}
