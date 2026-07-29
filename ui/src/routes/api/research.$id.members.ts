import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
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
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await researchRole(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })
        return json({ members: await listResearchMembers(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if ((await researchRole(user.id, params.id)) !== 'owner') {
          return json({ error: 'only the research owner can share it' }, { status: 403 })
        }
        const body = await parseBody(request, z.object({ email: z.string().email() }))
        if (body instanceof Response) return body
        const sql = await db()
        const rows = (await sql`select id from users where lower(email) = ${body.email.toLowerCase()}`) as unknown as Array<{ id: string }>
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
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const role = await researchRole(user.id, params.id)
        const body = await parseBody(request, z.object({ userId: z.string().uuid() }))
        if (body instanceof Response) return body
        if (role !== 'owner' && !(role === 'member' && body.userId === user.id)) {
          return json({ error: 'forbidden' }, { status: 403 })
        }
        await removeResearchMember(params.id, body.userId)
        await syncReportGrant(params.id, body.userId, false)
        return json({ members: await listResearchMembers(params.id) })
      },
    },
  },
})
