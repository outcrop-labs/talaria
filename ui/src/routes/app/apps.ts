// Shared types + API calls for the Apps page (Apps.svelte + its tabs).
import { errorMessage, getJson, postJson, postJsonOr, putJson } from '@/lib/fetch-json'
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
  /** The app's triggers (talaria.json), each with its newest runs. */
  triggers?: AppTriggerInfo[]
}
/** One run of an app's trigger, as talaria-app-triggers records it. */
export interface AppTriggerRun {
  state: 'queued' | 'running' | 'ok' | 'failed' | 'skipped'
  event: string | null
  /** The schedule's minute this run was for (cron only). */
  dueAt: string | null
  queuedAt: string
  startedAt: string | null
  finishedAt: string | null
  /** The handler's own result line. */
  detail: string | null
  error: string | null
}
/** A declared trigger: a schedule (UTC) with its next minute, or an event. */
export interface AppTriggerInfo {
  id: string
  cron?: string
  event?: string
  nextAt?: string | null
  /** Newest first. */
  runs: AppTriggerRun[]
}
/** `git ls-remote` against an app's recorded ref (GET ?updates=1, admin). */
export interface AppUpdateCheck {
  current: string | null
  latest: string | null
  available: boolean
  pinned: boolean
  /** The source was not asked: it is read with a workspace secret, and a
   *  check would spend a use of it. Update fetches with it. */
  notChecked?: string
  error?: string
  checkedAt: string
}
/** PUT { app, update: true } — on failure, `rolledBack` true: the app still
 *  runs `from`; false: the swap back failed and apps/<slug> holds `to`. */
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

/** Data an earlier install kept under the slug, from another repository (or
 *  an unrecorded one): the install answers 409 until it is told to adopt or
 *  wipe it. */
export interface KeptDataConflict {
  slug: string
  keptFrom: string | null
  installing: string
}
export interface InstallAnswer {
  error?: string
  /** On the 409: the kept data and the repository being installed. */
  conflict?: KeptDataConflict
  slug?: string
  keptData?: boolean
  sha?: string
  wipedData?: boolean
  build?: { ok: boolean; error?: string } | null
  buildError?: string
}

/** POST an install. A 409 is an answer: kept data the request must decide
 *  about (`conflict` names the two repositories). */
export const installApp = async (body: Record<string, unknown>): Promise<InstallAnswer> => {
  try {
    return await postJsonOr<InstallAnswer>('/api/admin/apps', body, [409])
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

export const post = async (body: Record<string, unknown>): Promise<{ error?: string; slug?: string }> => {
  const send = 'installUrl' in body ? postJson : putJson
  try {
    return await send<{ error?: string; slug?: string }>('/api/admin/apps', body)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}
