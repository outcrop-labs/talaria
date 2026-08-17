import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin, requirePerm, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { listProfiles, mountError, updateProfile } from '@/server/workbench'
import { logAudit } from '@/server/audit'

const Patch = z.object({
  slug: z.string().min(1).max(40),
  name: z.string().min(1).max(80).optional(),
  description: z.string().max(500).optional(),
  env: z.record(z.string(), z.string().max(500)).optional(),
  harnesses: z.array(z.string().max(40)).max(20).optional(),
  autoAttach: z.object({ departments: z.array(z.string().max(60)).max(20).optional(), roles: z.array(z.string().max(60)).max(20).optional() }).optional(),
  enabled: z.boolean().optional(),
  // ── admin-only below: these two reach the host, not just the sandbox ──
  image: z.string().max(200).optional(),
  mounts: z.array(z.string().max(300)).max(20).optional(),
})

/** The fields that render straight into the fleet's compose services. */
const INFRA_FIELDS = ['image', 'mounts'] as const

// Workbench profiles — the role-agnostic sandbox registry ('dev' seeded;
// designer/data/etc ride the same table). GET → any member (the Studio and
// agent views show attachment state); PUT → agents.manage, except the
// infrastructure fields, which are admin-only.
export const Route = defineApi('/api/workbench', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const profiles = await listProfiles()
    if (await hasPerm(user, 'agents.manage')) return json({ profiles })
    // A profile's env is injected straight into agent containers and is the
    // documented home for scoped credentials, so its VALUES are not member
    // -readable. Keys stay so the attachment UI can still explain itself.
    return json({
      profiles: profiles.map((p) => ({ ...p, env: Object.fromEntries(Object.keys(p.env ?? {}).map((k) => [k, '•••'])) })),
    })
  },
  PUT: async ({ request }) => {
    const gate = await requirePerm(request, 'agents.manage')
    if (gate instanceof Response) return gate
    let user = gate
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body
    const { slug, ...patch } = body

    // image and mounts become compose volumes / the image the sandbox runs as
    // root from — a host mount plus a fleet roll is host root. agents.manage is
    // grantable to non-admins, and the catalog entry promises its holders that
    // "infrastructure stays admin-only", so hold that line here.
    const infra = INFRA_FIELDS.filter((f) => patch[f] !== undefined)
    if (infra.length) {
      const admin = await requireAdmin(request)
      if (admin instanceof Response) return json({ error: `${infra.join(' and ')} are admin-only` }, { status: 403 })
      user = admin
    }
    for (const mount of patch.mounts ?? []) {
      const why = mountError(mount)
      if (why) return json({ error: `mount "${mount}" rejected: ${why}` }, { status: 400 })
    }

    if (!(await updateProfile(slug, patch))) return json({ error: 'unknown profile' }, { status: 404 })
    void logAudit({
      actor: actorOf(user),
      action: 'workbench.profile_update',
      targetType: 'workbench-profile',
      targetId: slug,
      targetLabel: patch.name ?? slug,
      // Env values are per-profile config that can carry credentials — the
      // trail records which names moved, never what they were set to.
      after: { ...patch, env: patch.env && Object.keys(patch.env) },
    })
    return json({ ok: true })
  },
})
