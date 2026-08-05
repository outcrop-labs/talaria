import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { clearSecret, clearUnreadable, secretHealth, UnknownSecretId } from '@/server/secret-health'
import { logAudit } from '@/server/audit'

// The secrets inventory. GET is a VIEW over the stores that own each value —
// presence, provenance and readability, never the value itself. DELETE clears
// one row's ciphertext, or every row that cannot be read.
//
// This is the in-app half of `scripts/reset.sh secrets`. The script clears
// everything sealed because a shell script cannot tell what is broken; this
// can, so it clears only that.
export const Route = createFileRoute('/api/admin/secrets')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireAdmin(request)
        if (gate instanceof Response) return gate
        return json(await secretHealth())
      },
      DELETE: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(
          request,
          z.union([z.object({ id: z.string().min(1).max(200) }), z.object({ unreadable: z.literal(true) })]),
        )
        if (body instanceof Response) return body

        if ('unreadable' in body) {
          const res = await clearUnreadable()
          void logAudit({
            actor: actorOf(user),
            action: 'secret.clear-unreadable',
            targetType: 'secrets',
            targetLabel: `${res.cleared.length} cleared`,
            after: res,
          })
          return json({ ok: true, ...res })
        }

        try {
          const changed = await clearSecret(body.id)
          // Audit the attempt either way: "an admin tried to clear this" is the
          // fact worth keeping, and a no-op still tells you someone was here.
          void logAudit({
            actor: actorOf(user),
            action: 'secret.clear',
            targetType: 'secret',
            targetId: body.id,
            after: { changed },
          })
          return json({ ok: true, changed })
        } catch (e) {
          if (e instanceof UnknownSecretId) return json({ error: 'unknown secret' }, { status: 404 })
          // Never echo the raw error: these paths sit next to key material.
          console.error('[admin.secrets] clear failed', body.id, e)
          return json({ error: 'could not clear that secret — see server logs' }, { status: 500 })
        }
      },
    },
  },
})
