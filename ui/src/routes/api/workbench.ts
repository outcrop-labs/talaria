import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { listProfiles, updateProfile } from '@/server/workbench'

const Patch = z.object({
  slug: z.string().min(1).max(40),
  name: z.string().min(1).max(80).optional(),
  description: z.string().max(500).optional(),
  image: z.string().max(200).optional(),
  env: z.record(z.string(), z.string().max(500)).optional(),
  mounts: z.array(z.string().max(300)).max(20).optional(),
  harnesses: z.array(z.string().max(40)).max(20).optional(),
  autoAttach: z.object({ departments: z.array(z.string().max(60)).max(20).optional(), roles: z.array(z.string().max(60)).max(20).optional() }).optional(),
  enabled: z.boolean().optional(),
})

// Workbench profiles — the role-agnostic sandbox registry ('dev' seeded;
// designer/data/etc ride the same table). GET → any member (the Studio and
// agent views show attachment state); PUT → agents.manage.
export const Route = createFileRoute('/api/workbench')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({ profiles: await listProfiles() })
      },
      PUT: async ({ request }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, Patch)
        if (body instanceof Response) return body
        const { slug, ...patch } = body
        await updateProfile(slug, patch)
        return json({ ok: true })
      },
    },
  },
})
