import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { addVersionIfChanged, getAgentDef, listVersions } from '@/server/agent-defs'

// GET → an agent definition's full version history (admin).
// POST { revertTo } → re-publish an old version's payload as a NEW version
// (history is append-only; a revert is itself a tracked change).
export const Route = createFileRoute('/api/fleet/defs/$id/versions')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })
        return json({ def, versions: await listVersions(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ revertTo: z.number().int().positive() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })
        const target = (await listVersions(def.id)).find((v) => v.version === parsed.data.revertTo)
        if (!target) return json({ error: 'version not found' }, { status: 404 })
        const res = await addVersionIfChanged(def.id, {
          soul: target.soul,
          config: target.config,
          note: `revert to v${target.version}`,
          createdBy: user.email ?? user.name ?? 'admin',
        })
        return json({ ok: true, ...res })
      },
    },
  },
})
