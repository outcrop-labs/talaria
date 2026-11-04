// Client view of the git hosts an operator chose (talaria-git-hosts): the
// rows Admin → Git hosts lists, edits and tests. The shapes mirror the
// routes' wire — and, like every credential surface here, carry no field
// that could hold a secret: a GitHub row says whether its key or token is
// SET, a Forgejo row names the workspace secret that holds its token.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export type GitHostKind = 'github' | 'forgejo'

export interface GitHostStatus {
  configured: boolean
  account: string | null
  error: string | null
}

export interface GitHost {
  id: string
  kind: GitHostKind
  kindLabel: string
  name: string
  baseUrl: string
  apiUrl: string | null
  repoCreationOrgs: string[]
  createdAt: string
  updatedAt: string
  /** A GitHub host's connection, redacted. */
  github?: {
    mode: 'app' | 'pat' | null
    app: { appId: string; installationIds: string[]; keySet: boolean }
    patSet: boolean
  }
  /** A Forgejo host's workspace secret, by name. */
  secret?: string | null
  /** What still names the host (the list only). */
  inUse?: { grants: number; jobs: number; repoRequests: number; installRequests: number; plugins: number; flows: number }
  /** The live answer (one host's read only). */
  status?: GitHostStatus
}

export interface GitHostTest {
  ok: boolean
  status: GitHostStatus
  repoCount: number | null
  repos: string[] | null
  reposError: string | null
}

export const GIT_HOSTS_KEY = ['git-hosts']
export const gitHostKey = (id: string) => ['git-host', id]

export const useGitHosts = () =>
  createQuery(() => ({
    queryKey: GIT_HOSTS_KEY,
    queryFn: (): Promise<{ hosts: GitHost[]; kinds: Array<{ id: GitHostKind; label: string }> }> =>
      getJson('/api/workbench/git-hosts'),
  }))

/** One host with its live status — the card's dot and its "connected as". */
export const useGitHost = (id: () => string) =>
  createQuery(() => ({
    queryKey: gitHostKey(id()),
    queryFn: async (): Promise<GitHost> => (await getJson<{ host: GitHost }>(`/api/workbench/git-hosts/${encodeURIComponent(id())}`)).host,
  }))

/** Refresh the list and one host after a write. */
export function useGitHostsRefresh() {
  const qc = useQueryClient()
  return async (id?: string) => {
    await qc.invalidateQueries({ queryKey: GIT_HOSTS_KEY })
    if (id) await qc.invalidateQueries({ queryKey: gitHostKey(id) })
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
}

/** Whether anything still names the host — the remove button's gate. */
export const hostInUse = (h: GitHost): boolean =>
  !!h.inUse && h.inUse.grants + h.inUse.jobs + h.inUse.repoRequests + h.inUse.installRequests + h.inUse.plugins > 0

/** A repo on a host, as the workbench routes spell it. */
export interface HostRepo {
  host: string
  repo: string
}

export const repoKey = (r: HostRepo) => `${r.host}:${r.repo}`
