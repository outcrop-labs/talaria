import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
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
  spaceBrain: z.object({ spaceId: Uuid, collectionId: Uuid.nullable() }).optional(),
})

const Post = z.union([
  z.object({ action: z.enum(['reindex', 'backfill']) }),
  // Model catalog for the picker. The candidate API key travels in a POST
  // body — NEVER a query string, where it would land in access/proxy logs.
  z.object({ models: z.string().min(1).max(40), key: z.string().max(500).nullish() }),
])

// Admin → Retrieval. GET → services health + backfill status + reranker
// providers/config + KB-space brain bindings. PUT → reranker config and/or a
// space↔brain binding. POST → kick a full backfill (detached), or
// { models, key? } → live model catalog for the picker.
export const Route = defineApi('/api/admin/rag', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
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
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Put)
    if (body instanceof Response) return body
    const actor = actorOf(user)
    if (body.reranker) {
      const cfg = await setRerankConfig({
        ...body.reranker,
        provider: body.reranker.provider as RerankProviderId | undefined,
      })
      void logAudit({ actor, action: 'rag.reranker', targetType: 'rag', targetId: 'reranker', after: { provider: cfg.provider, model: cfg.model } })
    }
    if (body.spaceBrain) {
      const sql = await db()
      await sql`update kb_spaces set rag_collection_id = ${body.spaceBrain.collectionId} where id = ${body.spaceBrain.spaceId}`
      // Existing docs move to their new home right away.
      void resyncSpaceDocs(body.spaceBrain.spaceId).catch(() => {})
      void logAudit({ actor, action: 'rag.space_brain', targetType: 'kb-space', targetId: body.spaceBrain.spaceId, after: { collectionId: body.spaceBrain.collectionId } })
    }
    return json({ rerank: { config: await rerankConfigPublic() } })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Post)
    if (body instanceof Response) return body
    if ('models' in body) {
      return json({ models: await rerankModels(body.models as RerankProviderId, body.key ?? null) })
    }
    // 'reindex' rebuilds collections in the current model's shape then
    // refills; 'backfill' refills in place. Both detach.
    const action = body.action
    if (action === 'reindex') void reindexAll().catch(() => {})
    else void backfillAll().catch(() => {})
    void logAudit({ actor: actorOf(user), action: `rag.${action}`, targetType: 'rag', targetId: action })
    return json({ started: true, action })
  },
})
