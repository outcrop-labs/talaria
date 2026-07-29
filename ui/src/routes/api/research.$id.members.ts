import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import {
  addResearchMember,
  listResearchMembers,
  removeResearchMember,
  researchArtifactFor,
  researchRole,
} from '@/server/research'
import { listEditors, setEditors } from '@/server/kb-perms'
import { addNotification } from '@/server/notifications'
import { db } from '@/server/db/pg'

/** Keep the report artifact's grants in step with run membership. */
async function syncReportGrant(runId: string, userId: string, present: boolean): Promise<void> {
  const artifactId = await researchArtifactFor(runId)
  if (!artifactId) return // report not written yet — completion grants members
  const grants = (await listEditors('artifact', artifactId)).filter(
    (g) => !(g.principalType === 'user' && g.principalId === userId),
  )
  if (present) grants.push({ principalType: 'user', principalId: userId, role: 'editor' })
  await setEditors('artifact', artifactId, grants)
}

// Multiplayer research, mirroring plan membership. GET → members (any member).
// POST { email } → share (owner only; grants the report, notifies). DELETE
// { userId } → unshare (owner, or a collaborator leaving).
export const Route = createFileRoute('/api/research/$id/members')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await researchRole(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })
        return json({ members: await listResearchMembers(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if ((await researchRole(user.id, params.id)) !== 'owner') {
          return json({ error: 'only the research owner can share it' }, { status: 403 })
        }
        const parsed = z.object({ email: z.string().email() }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const sql = await db()
        const rows = (await sql`select id from users where lower(email) = ${parsed.data.email.toLowerCase()}`) as unknown as Array<{ id: string }>
        if (!rows[0]) return json({ error: 'no user with that email' }, { status: 400 })
        if (rows[0].id === user.id) return json({ error: 'that is you' }, { status: 400 })
        await addResearchMember(params.id, rows[0].id)
        await syncReportGrant(params.id, rows[0].id, true)
        const [run] = (await sql`select question from research_runs where id = ${params.id}`) as unknown as Array<{ question: string }>
        void addNotification(rows[0].id, {
          kind: 'research-share',
          title: `${user.name ?? user.email ?? 'Someone'} shared research with you`,
          body: run?.question ?? '',
          href: `/research?r=${params.id}`,
        }).catch(() => {})
        return json({ members: await listResearchMembers(params.id) })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const role = await researchRole(user.id, params.id)
        const parsed = z.object({ userId: z.string().uuid() }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (role !== 'owner' && !(role === 'member' && parsed.data.userId === user.id)) {
          return json({ error: 'forbidden' }, { status: 403 })
        }
        await removeResearchMember(params.id, parsed.data.userId)
        await syncReportGrant(params.id, parsed.data.userId, false)
        return json({ members: await listResearchMembers(params.id) })
      },
    },
  },
})
