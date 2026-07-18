import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { logAudit } from '@/server/audit'
import { getStorageConfig, publicStorageConfig, setStorageConfig, testStorage } from '@/server/storage'
import { migrateStatus, migrateUploadsToS3, uploadStats } from '@/server/uploads'

const Body = z.object({
  mode: z.enum(['local', 's3']),
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
})

// Object storage (uploads blob store) config. GET → config (secret masked) +
// blob location stats + migration status. PUT → save config. POST → run a
// connection test or the local→bucket migration.
export const Route = createFileRoute('/api/admin/storage')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const [config, stats, migrate] = await Promise.all([publicStorageConfig(), uploadStats(), migrateStatus()])
        return json({ config, stats, migrate })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: parsed.error.issues[0]?.message ?? 'bad request' }, { status: 400 })
        const current = await getStorageConfig()
        const next = {
          ...parsed.data,
          endpoint: parsed.data.endpoint.trim().replace(/\/+$/, ''),
          secretAccessKey: parsed.data.secretAccessKey || current.secretAccessKey,
        }
        await setStorageConfig(next)
        void logAudit({
          actor: user.email ?? user.name ?? 'admin',
          action: 'settings.storage',
          targetType: 'settings',
          after: { mode: next.mode, endpoint: next.endpoint, bucket: next.bucket },
        })
        return json({ config: await publicStorageConfig() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const body = (await request.json().catch(() => ({}))) as { action?: string }
        if (body.action === 'test') {
          const result = await testStorage(await getStorageConfig())
          return json(result)
        }
        if (body.action === 'migrate') {
          try {
            const status = await migrateUploadsToS3()
            void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'storage.migrate', targetType: 'settings', after: { total: status.total } })
            return json({ migrate: status })
          } catch (e) {
            return json({ error: e instanceof Error ? e.message : 'migration failed' }, { status: 400 })
          }
        }
        return json({ error: 'unknown action' }, { status: 400 })
      },
    },
  },
})
