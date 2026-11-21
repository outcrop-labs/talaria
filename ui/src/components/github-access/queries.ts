// The reads every GitHub access surface shares, each under the one key the
// rest of the app already uses for it, so the overview, an agent's editor
// (on the page or in the agent's dialog), the Git hosts row and the host
// dialog read and invalidate the same cache entries. Call during component
// init, like any query hook.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import type { AgentGithubPermissions } from '@/lib/agent-github-permissions'
import { getJson, getList } from '@/lib/fetch-json'
import { gitHostAgentsKey } from '@/lib/git-host-agents'
import type { PermissionRequest } from '@/lib/git-permission-requests'
import { githubPermissionsKey, type RepoRequestLike } from '@/lib/github-access'
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'

export const PERMISSION_REQUESTS_KEY = ['permission-requests']
export const REPO_REQUESTS_KEY = ['repo-requests']

export const fetchGithubPermissions = (agentId: string): Promise<AgentGithubPermissions> =>
  getJson<AgentGithubPermissions>(`/api/workbench/github-permissions/${encodeURIComponent(agentId)}`)

/** One agent's GitHub permissions on every host it works on. */
export function useGithubPermissions(agentId: MaybeGetter<string>, enabled: MaybeGetter<boolean> = true) {
  return createQuery(() => ({
    queryKey: githubPermissionsKey(resolve(agentId)),
    queryFn: () => fetchGithubPermissions(resolve(agentId)),
    enabled: resolve(enabled) && resolve(agentId) !== '',
  }))
}

/** The open GitHub permission requests (admins). Polled: an agent waits on
 *  them, and a failed poll is an error, never "nothing waiting". */
export function usePermissionRequests(enabled: MaybeGetter<boolean> = true) {
  return createQuery(() => ({
    queryKey: PERMISSION_REQUESTS_KEY,
    queryFn: (): Promise<PermissionRequest[]> => getList<PermissionRequest>('/api/workbench/permission-requests', 'requests'),
    refetchInterval: 60_000,
    enabled: resolve(enabled),
  }))
}

/** One open new-repository request, as the Git hosts queue reads it. */
export interface RepoRequest extends RepoRequestLike {
  host: string
  agentModel: string
  why: string
  createdAt: string
}

/** The open new-repository requests (admins), decided in Git hosts. */
export function useRepoRequests(enabled: MaybeGetter<boolean> = true) {
  return createQuery(() => ({
    queryKey: REPO_REQUESTS_KEY,
    queryFn: (): Promise<RepoRequest[]> => getList<RepoRequest>('/api/workbench/repo-requests', 'requests'),
    refetchInterval: 60_000,
    enabled: resolve(enabled),
  }))
}

/** Everything an agent's GitHub access is read from, re-read after a write
 *  that changes it: its permissions, its repos, the requests (a removed
 *  grant may withdraw one), and each named host's agents read. */
export function useGithubAccessRefresh() {
  const qc = useQueryClient()
  return async (agentId: string, hosts: readonly string[] = []) => {
    await Promise.all([
      qc.invalidateQueries({ queryKey: githubPermissionsKey(agentId) }),
      qc.invalidateQueries({ queryKey: ['workbench-repos', agentId] }),
      qc.invalidateQueries({ queryKey: PERMISSION_REQUESTS_KEY }),
      ...hosts.map((h) => qc.invalidateQueries({ queryKey: gitHostAgentsKey(h) })),
    ])
  }
}
