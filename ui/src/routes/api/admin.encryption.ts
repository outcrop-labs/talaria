import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { db } from '@/server/db/pg'
import { rotateSecrets } from '@/server/secret-rotation'
import { logAudit } from '@/server/audit'

// Encryption status + one-click key rotation. Rotating re-generates the data key
// and re-encrypts every stored secret (provider keys, agent secrets, OAuth
// tokens) in a single pass — one action, no per-secret steps.
export const Route = createFileRoute('/api/admin/encryption')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const sql = await db()
        const active = (await sql`
          select version, created_at as "createdAt" from secret_keys where active order by version desc limit 1
        `) as unknown as Array<{ version: number; createdAt: string }>
        const counts = (await sql`
          select
            (select count(*) from llm_endpoints where api_key_cipher is not null)
            + (select count(*) from agent_secrets where value_enc is not null)
            + (select count(*) from google_connections)
            + (select count(*) from google_org_connection) as n
        `) as unknown as Array<{ n: number }>
        const rootSource = process.env.TALARIA_SECRET_KEY
          ? 'env:TALARIA_SECRET_KEY'
          : process.env.TALARIA_SECRET_KEY_FILE
            ? 'key-file'
            : 'env:AUTH_SECRET'
        return json({
          keyVersion: active[0]?.version ?? null,
          rotatedAt: active[0]?.createdAt ?? null,
          secretCount: Number(counts[0]?.n ?? 0),
          rootSource,
          algorithm: 'AES-256-GCM (envelope; post-quantum-safe symmetric)',
        })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const body = z
          .object({ newRootSecret: z.string().min(16).max(400).optional() })
          .safeParse(await request.json().catch(() => ({})))
        if (!body.success) return json({ error: 'new root secret must be at least 16 chars' }, { status: 400 })
        try {
          const res = await rotateSecrets(body.data.newRootSecret)
          void logAudit({
            actor: user.email ?? user.name ?? 'admin',
            action: 'encryption.rotate',
            targetType: 'secret_keys',
            targetLabel: `v${res.version}`,
            after: { reencrypted: res.reencrypted, rootRewrapped: res.rootRewrapped },
          })
          return json({ ok: true, ...res })
        } catch (e) {
          // Server log gets the real error; the client gets a generic line —
          // crypto failure messages must never risk echoing key material.
          console.error('[encryption.rotate]', e)
          return json({ error: 'rotation failed (no secrets changed) — see server logs' }, { status: 500 })
        }
      },
    },
  },
})
