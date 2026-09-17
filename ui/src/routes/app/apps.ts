// Shared types + API calls for the Apps page (Apps.svelte + its tabs).
import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
import type { AppBuildInfo } from '@/lib/apps'

export interface InstalledApp {
  slug: string
  name: string
  icon: string
  description: string
  version: string
  surfaces: { work?: string; manage?: string; settings?: string }
  mcp?: boolean
  enabled: boolean
  source: string | null
  build?: AppBuildInfo
}
export interface CatalogApp {
  slug: string
  name: string
  icon: string
  description: string
  repo: string
  author: string
  official: boolean
  version?: string
}
export interface AdminApps {
  apps: InstalledApp[]
  catalog: { apps: CatalogApp[]; error?: string } | null
  catalogUrl: string
}

export const fetchAdminApps = (withCatalog: boolean): Promise<AdminApps> =>
  getJson<AdminApps>(`/api/admin/apps${withCatalog ? '?catalog=1' : ''}`)

export const post = async (body: Record<string, unknown>): Promise<{ error?: string; slug?: string }> => {
  const send = 'installUrl' in body ? postJson : putJson
  try {
    return await send<{ error?: string; slug?: string }>('/api/admin/apps', body)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}
