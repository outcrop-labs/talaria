import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import {
  ensureBucket,
  getStorageConfig,
  internalTarget,
  publicStorageConfig,
  refuseDevSecret,
  replicaTarget,
  setStorageConfig,
  testStorage,
} from '@/server/storage'
import { migrateStatus, migrateUploadsToS3, syncStatus, syncUploadsToReplica, uploadStats } from '@/server/uploads'

const Target = {
  endpoint: z.string().max(300),
  region: z.string().max(60),
  bucket: z.string().max(200),
  accessKeyId: z.string().max(200),
  // Omitted or empty = keep the currently stored secret.
  secretAccessKey: z.string().max(400).optional(),
  pathStyle: z.boolean(),
  prefix: z
    .string()
    .max(120)
    .regex(/^$|^[a-zA-Z0-9._/-]+\/$/, 'prefix must end with /'),
}
const Body = z.object({
  mode: z.enum(['local', 'internal', 's3']),
  ...Target,
  replica: z.object({ enabled: z.boolean(), ...Target }),
})

// Object storage (uploads blob store) config. GET → config (secrets masked) +
// blob location stats + migration/sync status + the built-in bucket's endpoint.
// PUT → save config. POST → connection tests, local→bucket migration, or a
// full sync to the replica.
export const Route = defineApi('/api/admin/storage', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const [config, stats, migrate, sync] = await Promise.all([publicStorageConfig(), uploadStats(), migrateStatus(), syncStatus()])
    return json({ config, stats, migrate, sync, internal: { endpoint: internalTarget().endpoint, bucket: internalTarget().bucket } })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const current = await getStorageConfig()
    const next = {
      ...body,
      endpoint: body.endpoint.trim().replace(/\/+$/, ''),
      secretAccessKey: body.secretAccessKey || current.secretAccessKey,
      replica: {
        ...body.replica,
        endpoint: body.replica.endpoint.trim().replace(/\/+$/, ''),
        secretAccessKey: body.replica.secretAccessKey || current.replica.secretAccessKey,
      },
    }
    await setStorageConfig(next)
    void logAudit({
      actor: actorOf(user),
      action: 'settings.storage',
      targetType: 'settings',
      after: { mode: next.mode, endpoint: next.endpoint, bucket: next.bucket, replica: next.replica.enabled ? next.replica.bucket : 'off' },
    })
    return json({ config: await publicStorageConfig() })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({ action: z.enum(['test', 'test-replica', 'migrate', 'sync']).optional() }),
    )
    if (body instanceof Response) return body
    const cfg = await getStorageConfig()
    const audit = (action: string, after?: unknown) =>
      void logAudit({ actor: actorOf(user), action: `storage.${action}`, targetType: 'settings', after: after as never })
    try {
      if (body.action === 'test') {
        // Test what the current mode would actually use.
        if (cfg.mode === 'internal') {
          const t = internalTarget()
          refuseDevSecret(t)
          await ensureBucket(t)
          return json(await testStorage(t))
        }
        return json(await testStorage(cfg))
      }
      if (body.action === 'test-replica') {
        const replica = replicaTarget(cfg) ?? cfg.replica
        return json(await testStorage(replica))
      }
      if (body.action === 'migrate') {
        const status = await migrateUploadsToS3()
        audit('migrate', { total: status.total })
        return json({ migrate: status })
      }
      if (body.action === 'sync') {
        const status = await syncUploadsToReplica()
        audit('sync', { total: status.total })
        return json({ sync: status })
      }
    } catch (e) {
      return json({ error: e instanceof Error ? e.message : 'action failed' }, { status: 400 })
    }
    return json({ error: 'unknown action' }, { status: 400 })
  },
})
