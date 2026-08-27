// Shared types + API calls for the Apps page (Apps.svelte + its tabs).
import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'

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
  pending: string[]
  catalog: { apps: CatalogApp[]; error?: string } | null
  catalogUrl: string
}

// `if (!r.ok) return null` made a 500 resolve as a SUCCESSFUL query carrying
// null, so React Query never entered its error state and both consumers below
// fell through to their empty states: a broken read rendered "No apps
// installed", pixel-for-pixel identical to the honest 200-with-nothing. Non-2xx
// throws; empty and broken are different answers.
export const fetchAdminApps = (withCatalog: boolean): Promise<AdminApps> =>
  getJson<AdminApps>(`/api/admin/apps${withCatalog ? '?catalog=1' : ''}`)

export const post = async (body: Record<string, unknown>): Promise<{ error?: string; slug?: string; pendingBuild?: boolean }> => {
  // POST installs (the action); PUT writes config (enable/disable, catalog).
  // Both tabs render `error` in-band, so failures resolve rather than reject.
  const send = 'installUrl' in body ? postJson : putJson
  try {
    return await send<{ error?: string; slug?: string; pendingBuild?: boolean }>('/api/admin/apps', body)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}
