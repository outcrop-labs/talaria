import { createQuery } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'

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

const post = (url: string, body: unknown) =>
  fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, credentials: 'same-origin', body: JSON.stringify(body) })

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

export const createTeam = (name: string) => post('/api/teams', { name }).then((r) => r.json())
export const addTeamMember = (teamId: string, email: string, role: TeamRole) =>
  post(`/api/teams/${teamId}/members`, { email, role })
export const removeTeamMember = (teamId: string, userId: string) =>
  fetch(`/api/teams/${teamId}/members`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ userId }),
  })
