// Shared types + API calls for the Apps page (Apps.svelte + its tabs).
import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
import type { AppBuildInfo } from '@/lib/apps'

/** Where an app installed from git came from, and the commit it runs. */
export interface AppGitSource {
  url: string
  host: string
  repo: string
  /** The ref install or the last update asked for; null is the default branch. */
  ref: string | null
  /** Null only for an app installed before commits were recorded. */
  sha: string | null
  previousSha: string | null
  installedAt: string | null
  updatedAt: string | null
}
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
  git?: AppGitSource | null
  build?: AppBuildInfo
  /** talaria.json fails the manifest rules: why. Never enabled; removable. */
  broken?: string
}
/** `git ls-remote` against an app's recorded ref (GET ?updates=1, admin). */
export interface AppUpdateCheck {
  current: string | null
  latest: string | null
  available: boolean
  pinned: boolean
  error?: string
  checkedAt: string
}
/** PUT { app, update: true } — `rolledBack`: the app still runs `from`. */
export interface AppUpdateResult {
  ok: boolean
  slug: string
  from: string | null
  to: string | null
  ref: string | null
  upToDate: boolean
  rolledBack: boolean
  error?: string
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
  updates: Record<string, AppUpdateCheck> | null
}

export const fetchAdminApps = (withCatalog: boolean): Promise<AdminApps> =>
  getJson<AdminApps>(`/api/admin/apps${withCatalog ? '?catalog=1' : ''}`)

export const fetchAppUpdates = async (): Promise<Record<string, AppUpdateCheck>> =>
  (await getJson<AdminApps>('/api/admin/apps?updates=1')).updates ?? {}

export const shortSha = (sha: string | null | undefined): string => (sha ? sha.slice(0, 7) : '—')

export const post = async (body: Record<string, unknown>): Promise<{ error?: string; slug?: string }> => {
  const send = 'installUrl' in body ? postJson : putJson
  try {
    return await send<{ error?: string; slug?: string }>('/api/admin/apps', body)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}
