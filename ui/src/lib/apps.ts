// Client registry for Talaria apps: which apps are enabled (server truth) and
// how to load their compiled surface modules (build-time glob, code-split per
// app — an app's bundle only downloads when one of its surfaces renders).
import { useQuery } from '@tanstack/react-query'
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
  return useQuery({
    queryKey: ['apps'],
    queryFn: async (): Promise<AppManifest[]> => {
      const r = await fetch('/api/apps', { credentials: 'same-origin' })
      if (!r.ok) return []
      const j = (await r.json()) as { apps?: AppManifest[] }
      return j.apps ?? []
    },
    staleTime: 60_000,
  })
}

const LOADERS = import.meta.glob('../../../apps/*/app.tsx') as Record<string, () => Promise<{ default: AppSurfaces }>>

export function appLoader(slug: string): (() => Promise<{ default: AppSurfaces }>) | null {
  const entry = Object.entries(LOADERS).find(([p]) => /apps\/([^/]+)\//.exec(p)?.[1] === slug)
  return entry?.[1] ?? null
}
