import { useQuery } from '@tanstack/react-query'
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

const post = (url: string, body: unknown) =>
  fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, credentials: 'same-origin', body: JSON.stringify(body) })

export function useTeams() {
  return useQuery({
    queryKey: ['teams'],
    queryFn: (): Promise<Team[]> => getList<Team>('/api/teams', 'teams'),
  })
}

export function useTeamMembers(teamId: string | null) {
  return useQuery({
    queryKey: ['team-members', teamId],
    enabled: !!teamId,
    queryFn: (): Promise<TeamMember[]> => getList<TeamMember>(`/api/teams/${teamId}/members`, 'members'),
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
