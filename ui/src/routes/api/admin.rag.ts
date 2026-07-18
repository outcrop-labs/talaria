import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { backfillAll, backfillStatus, ragHealth } from '@/server/retrieval/backfill'
import { reindexAll, reindexStatus, retrievalUpgradeStatus } from '@/server/retrieval/migrate'
import {
  RERANK_PROVIDERS,
  rerankConfigPublic,
  rerankModels,
  setRerankConfig,
  type RerankProviderId,
} from '@/server/retrieval/rerank'
import { resyncSpaceDocs } from '@/server/retrieval/sources'
import { logAudit } from '@/server/audit'
import { db } from '@/server/db/pg'

const PROVIDER_IDS = ['off', ...RERANK_PROVIDERS.map((p) => p.id)] as [string, ...string[]]

const Put = z.object({
  reranker: z
    .object({
      provider: z.enum(PROVIDER_IDS).optional(),
      url: z.string().max(500).nullish(),
      model: z.string().max(200).nullish(),
      apiKey: z.string().max(500).nullish(),
      candidates: z.number().int().min(5).max(100).optional(),
    })
    .optional(),
  /** Bind a KB space to a brain (custom collection); null unbinds. Existing
   *  docs re-route immediately. */
  spaceBrain: z.object({ spaceId: z.string().uuid(), collectionId: z.string().uuid().nullable() }).optional(),
})

// Admin → Retrieval. GET → services health + backfill status + reranker
// providers/config + KB-space brain bindings. PUT → reranker config and/or a
// space↔brain binding. POST → kick a full backfill (detached).
// GET ?models=<provider>[&key=] → live model catalog for the picker.
export const Route = createFileRoute('/api/admin/rag')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const url = new URL(request.url)
        const modelsFor = url.searchParams.get('models')
        if (modelsFor) {
          return json({ models: await rerankModels(modelsFor as RerankProviderId, url.searchParams.get('key')) })
        }
        const sql = await db()
        const spaces = (await sql`
          select id, name, rag_collection_id as "collectionId" from kb_spaces order by name asc
        `) as unknown as Array<{ id: string; name: string; collectionId: string | null }>
        return json({
          health: await ragHealth(),
          backfill: await backfillStatus(),
          upgrade: await retrievalUpgradeStatus().catch(() => null),
          reindex: await reindexStatus(),
          rerank: { providers: RERANK_PROVIDERS, config: await rerankConfigPublic() },
          spaces,
        })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Put.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const actor = user.email ?? user.name ?? 'admin'
        if (parsed.data.reranker) {
          const cfg = await setRerankConfig({
            ...parsed.data.reranker,
            provider: parsed.data.reranker.provider as RerankProviderId | undefined,
          })
          void logAudit({ actor, action: 'rag.reranker', targetType: 'rag', targetId: 'reranker', after: { provider: cfg.provider, model: cfg.model } })
        }
        if (parsed.data.spaceBrain) {
          const sql = await db()
          await sql`update kb_spaces set rag_collection_id = ${parsed.data.spaceBrain.collectionId} where id = ${parsed.data.spaceBrain.spaceId}`
          // Existing docs move to their new home right away.
          void resyncSpaceDocs(parsed.data.spaceBrain.spaceId).catch(() => {})
          void logAudit({ actor, action: 'rag.space_brain', targetType: 'kb-space', targetId: parsed.data.spaceBrain.spaceId, after: { collectionId: parsed.data.spaceBrain.collectionId } })
        }
        return json({ rerank: { config: await rerankConfigPublic() } })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        // { action: 'reindex' } rebuilds collections in the current model's
        // shape then refills; default (or 'backfill') refills in place.
        const body = (await request.json().catch(() => ({}))) as { action?: string }
        const action = body.action === 'reindex' ? 'reindex' : 'backfill'
        if (action === 'reindex') void reindexAll().catch(() => {})
        else void backfillAll().catch(() => {})
        void logAudit({ actor: user.email ?? 'admin', action: `rag.${action}`, targetType: 'rag', targetId: action })
        return json({ started: true, action })
      },
    },
  },
})
