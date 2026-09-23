import { resolve, type MaybeGetter } from '@/lib/reactive-arg'
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getJson, getList, patchJson, postJson, putJson } from '@/lib/fetch-json'
import { toastError } from '@/lib/toast.svelte'

export type TeamRole = 'owner' | 'member'
export interface Team {
  id: string
  name: string
  /** Caller's standing; `""` when listing every org team and they are not on it. */
  role: TeamRole | ''
  memberCount: number
  agentCount: number
  description?: string | null
  createdAt: string
}
export interface TeamMember {
  userId: string
  email: string | null
  name: string | null
  role: TeamRole
}
export interface TeamAgent {
  agentModel: string
  label?: string | null
}
export interface TeamDirectoryEntry {
  id: string
  name: string
  memberCount: number
  agentCount: number
}
export interface TeamAccess {
  deniedViews: string[]
  allowedManageViews: string[]
  permissions: Record<string, boolean>
}

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */

export function useTeams() {
  return createQuery(() => ({
    queryKey: ['teams'],
    queryFn: (): Promise<Team[]> => getList<Team>('/api/teams', 'teams'),
  }))
}

/** Every org team. The Manage page; gated by the `/teams` view. */
export function useTeamsAll() {
  return createQuery(() => ({
    queryKey: ['teams', 'all'],
    queryFn: (): Promise<Team[]> => getList<Team>('/api/teams?all=1', 'teams'),
  }))
}

/** Teams by id + name + counts for share pickers — any signed-in caller.
 *
 *  One hook, one cache entry, one shape: five surfaces hand-rolled this read
 *  and each declared a different slice of the row (`{ teams: [{ id, name }] }`
 *  in channel settings, `{ id, name, memberCount }` in KB sharing, the whole
 *  row for MCP access) under a single `['teams-directory']` key — the same
 *  entry typed three ways. The key hangs off `['teams']` so the membership
 *  writes that invalidate the team lists invalidate this with them.
 *
 *  `enabled` follows `useTeamAccess`: a picker that only reads while its modal
 *  is open passes that gate, rather than the hook guessing on its behalf.
 *  30s stale mirrors `useUsers` — the directory moves only on a membership
 *  write, which is rare next to a modal reopen. */
export function useTeamsDirectory(enabled: MaybeGetter<boolean> = true) {
  return createQuery(() => ({
    queryKey: ['teams', 'directory'],
    enabled: resolve(enabled),
    queryFn: (): Promise<TeamDirectoryEntry[]> => getList<TeamDirectoryEntry>('/api/teams/directory', 'teams'),
    staleTime: 30_000,
  }))
}

export function useTeamMembers(teamId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(teamId)
    return {
      queryKey: ['team-members', id],
      enabled: !!id,
      queryFn: (): Promise<TeamMember[]> => getList<TeamMember>(`/api/teams/${id}/members`, 'members'),
    }
  })
}

/** Agent roster. Reads GET /api/teams/{id} (member or `/teams` view) so the
 *  Manage list can show agents on teams the caller is not on; writes still
 *  hit `/agents`. */
export function useTeamAgents(teamId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(teamId)
    return {
      queryKey: ['team-agents', id],
      enabled: !!id,
      queryFn: async (): Promise<TeamAgent[]> => {
        const data = await getJson<{ agents: TeamAgent[] }>(`/api/teams/${id}`)
        return data.agents
      },
    }
  })
}

/** Admin-only view denials, manage grants, and perm overrides. */
export function useTeamAccess(teamId: MaybeGetter<string | null>, enabled: MaybeGetter<boolean> = true) {
  return createQuery(() => {
    const id = resolve(teamId)
    return {
      queryKey: ['team-access', id],
      enabled: !!id && resolve(enabled),
      queryFn: async (): Promise<TeamAccess> => {
        const data = await getJson<{ access: TeamAccess }>(`/api/teams/${id}/access`)
        return data.access
      },
    }
  })
}

export const createTeam = (name: string) => postJson<{ team: Team }>('/api/teams', { name })
export const addTeamMember = (teamId: string, email: string, role: TeamRole) =>
  postJson<{ ok: true }>(`/api/teams/${teamId}/members`, { email, role })
export const removeTeamMember = (teamId: string, userId: string) =>
  // The call site fires and forgets (`.then(refresh)`, no catch), so a refused
  // remove is surfaced here rather than left as an unhandled rejection.
  delJson<{ ok: true }>(`/api/teams/${teamId}/members`, { userId }).catch((e: unknown) =>
    toastError('Remove failed', e),
  )
export const renameTeam = (teamId: string, name: string) =>
  patchJson<{ ok: true }>(`/api/teams/${teamId}`, { name })
export const patchTeam = (teamId: string, patch: { name?: string; description?: string | null }) =>
  patchJson<{ ok: true }>(`/api/teams/${teamId}`, patch)
export const deleteTeam = (teamId: string) => delJson<{ ok: true }>(`/api/teams/${teamId}`)
export const addTeamAgent = (teamId: string, model: string) =>
  postJson<{ ok: true }>(`/api/teams/${teamId}/agents`, { model })
export const removeTeamAgent = (teamId: string, model: string) =>
  delJson<{ ok: true }>(`/api/teams/${teamId}/agents`, { model })
export const putTeamAccess = (
  teamId: string,
  body: {
    deniedViews?: string[]
    allowedManageViews?: string[]
    permissions?: Record<string, boolean | null>
  },
) => putJson<{ access: TeamAccess }>(`/api/teams/${teamId}/access`, body)
