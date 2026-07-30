import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { deleteCustomHarness, listHarnessDefs, upsertCustomHarness } from '@/server/workbench-harnesses'

const Definition = z.object({
  slug: z.string().regex(/^[a-z0-9][a-z0-9-]*$/).max(40),
  label: z.string().min(1).max(60),
  description: z.string().max(300).optional(),
  auth: z.union([z.literal('gateway'), z.object({ provider: z.string().min(1).max(40), envVar: z.string().min(1).max(60) })]),
  env: z.record(z.string().max(60), z.string().max(300)).optional(),
  modelPrefix: z.string().max(40).optional(),
  invoke: z.string().min(1).max(500),
  jsonInvoke: z.string().max(500).optional(),
  mcpServe: z.object({ command: z.string().min(1).max(120), args: z.array(z.string().max(120)).max(10) }).optional(),
  mcpConfig: z.object({ format: z.enum(['claude-json', 'opencode-json']), filename: z.string().regex(/^[a-z0-9][a-z0-9.-]*$/).max(60) }).optional(),
  guide: z.string().min(1).max(2000),
  install: z.object({ npm: z.array(z.string().max(120)).max(10).optional(), commands: z.array(z.string().max(300)).max(10).optional(), notes: z.string().max(500).optional() }).optional(),
})

// The harness registry. GET → merged definitions with sources (any member —
// grounds the per-agent dropdowns). PUT → register/replace a CUSTOM
// definition (declarative JSON, no code); DELETE ?slug= removes one.
// Builtin/app-shipped entries can be shadowed by slug but never deleted.
export const Route = createFileRoute('/api/workbench/harnesses')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({ harnesses: await listHarnessDefs() })
      },
      PUT: async ({ request }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, Definition)
        if (body instanceof Response) return body
        await upsertCustomHarness(body.slug, body, actorOf(user))
        return json({ harnesses: await listHarnessDefs() })
      },
      DELETE: async ({ request }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const slug = new URL(request.url).searchParams.get('slug')
        if (!slug) return json({ error: 'slug required' }, { status: 400 })
        await deleteCustomHarness(slug)
        return json({ harnesses: await listHarnessDefs() })
      },
    },
  },
})
