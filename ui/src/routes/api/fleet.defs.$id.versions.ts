import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requirePerm } from '@/server/api-guard'
import { addVersionIfChanged, getAgentDef, listVersions } from '@/server/agent-defs'

// GET → an agent definition's full version history (admin).
// POST { revertTo } → re-publish an old version's payload as a NEW version
// (history is append-only; a revert is itself a tracked change).
export const Route = defineApi('/api/fleet/defs/$id/versions', {
  GET: async ({ request, params }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const def = await getAgentDef(params.id)
    if (!def) return json({ error: 'not found' }, { status: 404 })
    return json({ def, versions: await listVersions(params.id) })
  },
  POST: async ({ request, params }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ revertTo: z.number().int().positive() }))
    if (body instanceof Response) return body
    const def = await getAgentDef(params.id)
    if (!def) return json({ error: 'not found' }, { status: 404 })
    const target = (await listVersions(def.id)).find((v) => v.version === body.revertTo)
    if (!target) return json({ error: 'version not found' }, { status: 404 })
    const res = await addVersionIfChanged(def.id, {
      soul: target.soul,
      config: target.config,
      note: `revert to v${target.version}`,
      createdBy: user.email ?? user.name ?? 'admin',
    })
    return json({ ok: true, ...res })
  },
})
