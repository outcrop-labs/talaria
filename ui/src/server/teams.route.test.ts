// The team rename/delete route, tested from src/server/ because vitest.config.ts
// excludes `src/routes/**` from COLLECTION (a dot is a path separator there, so
// `api/mcp.test.ts` is a handler, not a suite) — same home and reason as
// realtime.routes.test.ts. The property under test is the one the route exists
// to enforce: A TEAM MUTATION IS OWNER-ONLY. `renameTeam`/`deleteTeam` in
// ./teams are bare writes with no permission check of their own, so the gate
// here is the only thing standing between a member and rewriting or removing
// a whole team — and its member list with it.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ApiHandler } from '@/server/api-route'

const session = vi.hoisted(() => ({ getSessionUser: vi.fn() }))
const teams = vi.hoisted(() => ({
  teamRole: vi.fn(),
  renameTeam: vi.fn(),
  deleteTeam: vi.fn(),
}))
const audit = vi.hoisted(() => ({ logAudit: vi.fn() }))

vi.mock('@/server/auth/session', () => session)
vi.mock('@/server/teams', () => teams)
vi.mock('@/server/audit', () => audit)

// The guard itself stays real so requireUser/parseBody run as shipped; only the
// session lookup is stubbed.
import { Route } from '@/routes/api/teams.$id'

const signedIn = { id: 'u1', email: 'u1@example.com', name: 'One', role: 'member' }

function call(handler: ApiHandler | undefined, url: string, params: Record<string, string>, init?: RequestInit): Promise<Response> {
  expect(handler).toBeDefined()
  return Promise.resolve(handler!({ request: new Request(url, init), params }))
}

beforeEach(() => {
  vi.clearAllMocks()
  session.getSessionUser.mockResolvedValue(signedIn)
})

describe('PATCH /api/teams/:id (rename)', () => {
  it('refuses an unauthenticated caller', async () => {
    session.getSessionUser.mockResolvedValue(null)
    const res = await call(Route.handlers.PATCH, 'http://t/api/teams/t1', { id: 't1' }, { method: 'PATCH', body: '{"name":"New"}' })
    expect(res.status).toBe(401)
    expect(teams.renameTeam).not.toHaveBeenCalled()
  })

  it('refuses a mere member without renaming', async () => {
    teams.teamRole.mockResolvedValue('member')
    const res = await call(Route.handlers.PATCH, 'http://t/api/teams/t1', { id: 't1' }, { method: 'PATCH', body: '{"name":"New"}' })
    expect(res.status).toBe(403)
    expect(teams.renameTeam).not.toHaveBeenCalled()
  })

  it('rejects a blank name for the owner, matching create’s validation', async () => {
    teams.teamRole.mockResolvedValue('owner')
    const res = await call(Route.handlers.PATCH, 'http://t/api/teams/t1', { id: 't1' }, { method: 'PATCH', body: '{"name":""}' })
    expect(res.status).toBe(400)
    expect(teams.renameTeam).not.toHaveBeenCalled()
  })

  it('renames for the owner and audits it', async () => {
    teams.teamRole.mockResolvedValue('owner')
    const res = await call(Route.handlers.PATCH, 'http://t/api/teams/t1', { id: 't1' }, { method: 'PATCH', body: '{"name":"New"}' })
    expect(res.status).toBe(200)
    expect(teams.renameTeam).toHaveBeenCalledWith('t1', 'New')
    expect(audit.logAudit).toHaveBeenCalledWith(expect.objectContaining({ action: 'team.rename', targetId: 't1' }))
  })
})

describe('DELETE /api/teams/:id', () => {
  it('refuses a mere member without deleting', async () => {
    teams.teamRole.mockResolvedValue('member')
    const res = await call(Route.handlers.DELETE, 'http://t/api/teams/t1', { id: 't1' }, { method: 'DELETE' })
    expect(res.status).toBe(403)
    expect(teams.deleteTeam).not.toHaveBeenCalled()
  })

  it('deletes for the owner and audits it', async () => {
    teams.teamRole.mockResolvedValue('owner')
    const res = await call(Route.handlers.DELETE, 'http://t/api/teams/t1', { id: 't1' }, { method: 'DELETE' })
    expect(res.status).toBe(200)
    expect(teams.deleteTeam).toHaveBeenCalledWith('t1')
    expect(audit.logAudit).toHaveBeenCalledWith(expect.objectContaining({ action: 'team.delete', targetId: 't1' }))
  })
})
