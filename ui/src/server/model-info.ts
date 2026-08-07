// Human-readable model identity, populated automatically. OpenRouter's public
// catalog (no key) carries a pretty name + description for essentially every
// major model; we match registered ids against it the same way the price
// oracle does (full id for slashed ids, unambiguous suffix for bare ones) and
// serve a display label + a one-line "what it's good at" blurb. Unknown models
// (e.g. self-hosted) simply have no blurb — nothing is invented.
//
// Blurbs additionally get ONE rewrite pass in the org's voice (task-oriented:
// what it's good at, when to pick it) through the `blurb-writer` harness, cached
// in model_blurbs; registered models without a rewrite get theirs on the next
// sweep, so new models arrive speaking the workspace's language automatically.
import { db } from './db/pg'
import { gatewayModels } from './llm-gateway'
import { blurbWriterHarness } from './harness/defs/blurb-writer'
import { runHarness } from './harness/run'
import { orgProfile } from './org'
import { NATIVE_BASE } from './provider-catalog'

export interface ModelInfo {
  /** Pretty display name, e.g. "Qwen: Qwen3 14B". */
  label: string
  /** First sentence of the catalog description, clamped. */
  blurb: string
}

interface Catalog {
  byId: Map<string, ModelInfo>
  bySuffix: Map<string, ModelInfo[]>
}

const normalize = (s: string) => s.toLowerCase().replace(/\./g, '-')

/** One line out of a marketing paragraph: first sentence, clamped. */
const toBlurb = (description: string): string => {
  const first = description.replace(/\s+/g, ' ').trim().split(/(?<=\.)\s/)[0] ?? ''
  return first.length > 160 ? `${first.slice(0, 157)}…` : first
}

let cache: { at: number; catalog: Catalog } | null = null
const TTL_MS = 6 * 60 * 60_000

async function catalog(): Promise<Catalog | null> {
  if (cache && Date.now() - cache.at < TTL_MS) return cache.catalog
  try {
    const r = await fetch(`${NATIVE_BASE['openrouter']}/models`, { signal: AbortSignal.timeout(10_000) })
    if (!r.ok) throw new Error(`models ${r.status}`)
    const j = (await r.json()) as { data?: Array<{ id?: string; name?: string; description?: string }> }
    const byId = new Map<string, ModelInfo>()
    const bySuffix = new Map<string, ModelInfo[]>()
    for (const m of j.data ?? []) {
      if (!m.id || !m.name) continue
      const info: ModelInfo = { label: m.name, blurb: toBlurb(m.description ?? '') }
      byId.set(normalize(m.id), info)
      const [, ...rest] = m.id.split('/')
      const suffix = normalize(rest.join('/'))
      if (suffix) bySuffix.set(suffix, [...(bySuffix.get(suffix) ?? []), info])
    }
    cache = { at: Date.now(), catalog: { byId, bySuffix } }
    return cache.catalog
  } catch {
    return cache?.catalog ?? null
  }
}

/** Raw public-catalog info for one id, or null when nothing matches cleanly. */
async function catalogInfo(modelId: string): Promise<ModelInfo | null> {
  const cat = await catalog()
  if (!cat) return null
  if (modelId.includes('/')) return cat.byId.get(normalize(modelId)) ?? null
  const candidates = cat.bySuffix.get(normalize(modelId)) ?? []
  return candidates.length === 1 ? candidates[0]! : null // ambiguous bare id → no guess
}

/** Info for one registered model id: catalog label + the org-voice blurb when
 *  it's been written, else the raw catalog blurb. Null when nothing matches. */
export async function modelInfo(modelId: string): Promise<ModelInfo | null> {
  const info = await catalogInfo(modelId)
  if (!info) return null
  const sql = await db()
  const rows = (await sql`select blurb from model_blurbs where model_id = ${modelId}`) as unknown as Array<{ blurb: string }>
  return rows[0] ? { label: info.label, blurb: rows[0].blurb } : info
}

// ── The org-voice rewrite pass ───────────────────────────────────────────────

/** Rewrite catalog blurbs for registered models that don't have one yet —
 *  one batched completion, results cached in model_blurbs. Returns how many
 *  were written. Bounded per pass; failures just wait for the next sweep. */
export async function rewritePendingBlurbs(batch = 10): Promise<number> {
  const sql = await db()
  const bare = (await gatewayModels()).filter((m) => !m.qualified).map((m) => m.id)
  if (bare.length === 0) return 0
  const done = new Set(
    ((await sql`select model_id as id from model_blurbs where model_id in ${sql(bare)}`) as unknown as Array<{ id: string }>).map(
      (r) => r.id,
    ),
  )
  const pending: Array<{ id: string; name: string; description: string }> = []
  for (const id of bare) {
    if (done.has(id) || pending.length >= batch) continue
    const info = await catalogInfo(id)
    if (info?.blurb) pending.push({ id, name: info.label, description: info.blurb })
  }
  if (pending.length === 0) return 0

  const org = await orgProfile()
  // Model resolution, the prompt, the extractor, the repair turn and the guard
  // pass all live in the harness now. What is left here is what this file is
  // actually for: which models are pending, and writing the rows.
  const { value } = await runHarness(blurbWriterHarness, { orgName: org.name, models: pending }, { caller: 'platform:blurb-writer' })
  if (!value) return 0

  // A PARTIAL batch is still a good pass. The models this sweep missed stay
  // pending and come back around in ten minutes, which is why the lookup is per
  // pending id rather than an iteration over whatever the model returned.
  //
  // THE KEY-RESOLUTION PASS THAT USED TO SIT HERE IS GONE, and its absence is
  // the point. `blurbsByCandidateId` re-keyed a reply the model had keyed by
  // display name ("Qwen3 14B" for `qwen3-14b`) by normalizing ids and names
  // until something matched. That rescued the batch and, in doing so, hid the
  // fact that the model had not answered the question the prompt asked: nothing
  // was re-asked, nothing was recorded, and the harness reported a perfect
  // contract. The rule now lives on `blurbWriterHarness.output.verify`, where a
  // wrong key is a repairable contract failure and lands on the `harness_runs`
  // row — so every key in `value` is one of `pending`'s ids by the time it gets
  // here and there is nothing left to resolve.
  let written = 0
  for (const p of pending) {
    const blurb = value[p.id]
    // Still guarded, because the contract deliberately tolerates a blank line in
    // an otherwise good batch (one over-long or empty sentence must not cost the
    // other nine). A blank blurb is worse than the catalog line it would replace.
    if (typeof blurb !== 'string' || !blurb.trim()) continue
    await sql`
      insert into model_blurbs (model_id, blurb) values (${p.id}, ${blurb.trim().slice(0, 200)})
      on conflict (model_id) do update set blurb = excluded.blurb
    `
    written++
  }
  return written
}

// Opportunistic: any catalog read may kick a rewrite pass for new models —
// throttled, detached, never blocking the request.
let lastSweep = 0
export function maybeRewriteBlurbs(): void {
  if (Date.now() - lastSweep < 10 * 60_000) return
  lastSweep = Date.now()
  void rewritePendingBlurbs().catch(() => {})
}
