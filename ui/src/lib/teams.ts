import { createQuery } from '@tanstack/svelte-query'
import { delJson, errorMessage, getList, patchJson, postJson } from '@/lib/fetch-json'
import { pushToast } from '@/lib/toast.svelte'

export type TeamRole = 'owner' | 'member'
export interface Team {
  id: string
  name: string
  role: TeamRole
  memberCount: number
  createdAt: string
}
export interface TeamMember {
  userId: string
  email: string | null
  name: string | null
  role: TeamRole
}

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export function useTeams() {
  return createQuery(() => ({
    queryKey: ['teams'],
    queryFn: (): Promise<Team[]> => getList<Team>('/api/teams', 'teams'),
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

export const createTeam = (name: string) => postJson<{ team: Team }>('/api/teams', { name })
export const addTeamMember = (teamId: string, email: string, role: TeamRole) =>
  postJson<{ ok: true }>(`/api/teams/${teamId}/members`, { email, role })
export const removeTeamMember = (teamId: string, userId: string) =>
  // The call site fires and forgets (`.then(refresh)`, no catch), so a refused
  // remove is surfaced here rather than left as an unhandled rejection.
  delJson<{ ok: true }>(`/api/teams/${teamId}/members`, { userId }).catch((e: unknown) =>
    pushToast({ title: 'Remove failed', body: errorMessage(e), tone: 'danger' }),
  )
export const renameTeam = (teamId: string, name: string) =>
  patchJson<{ ok: true }>(`/api/teams/${teamId}`, { name })
export const deleteTeam = (teamId: string) => delJson<{ ok: true }>(`/api/teams/${teamId}`)
