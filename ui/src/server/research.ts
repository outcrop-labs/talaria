// Research runs — Perplexity-grade cited research, Talaria-native.
//
// Three modes, mapped to depth budgets and sonar horsepower:
//   recon       one search pass, minutes — a cited answer
//   brief       planned queries + one synthesis round — a briefing document
//   expedition  iterative plan → search → gap-check rounds — a deep report
//
// Architecture: the pipeline runs SERVER-SIDE, detached from any chat context.
// The requesting agent's own persona (via its persona gateway) does the
// thinking — query planning, gap analysis, synthesis — so a marketing agent
// researches like a marketer; each stage gets a fresh, bounded prompt so raw
// search dumps never swell a conversation window. Search stages run Perplexity
// sonar models through the org LLM gateway (routing + keys + metering), which
// return content WITH source lists; we keep a global, deduped citation
// registry and require [n] markers in the final document.
//
// The report is a doc artifact (versioned, shareable, exportable) linked to
// the run; completion notifies the requester and indexes into the activity
// brain so future chats/plans can retrieve it.
import { parseAgentStream } from '@/lib/sse-parse'
import { agentCategoryFolder, attachArtifact, createArtifact, saveArtifact } from './artifacts'
import { db } from './db/pg'
import { describeAgent, proxyChat } from './gateway'
import { buildUpstream, fetchUpstream, gatewayModels, recordGatewayUsage, resolveRoute } from './llm-gateway'
import { resolveRoleModel } from './model-roles'
import { addNotification } from './notifications'
import { indexActivity } from './retrieval/sources'
import { estimateTokens, recordUsage } from './usage'

export type ResearchMode = 'recon' | 'brief' | 'expedition'
export type ResearchStatus = 'queued' | 'running' | 'done' | 'error'

export interface ResearchRun {
  id: string
  ownerUserId: string | null
  requestedBy: string
  agentModel: string
  mode: ResearchMode
  question: string
  status: ResearchStatus
  phase: string | null
  artifactId: string | null
  error: string | null
  stats: Record<string, number>
  createdAt: string
  updatedAt: string
  completedAt: string | null
}

export interface ResearchSource {
  idx: number
  url: string
  title: string | null
  snippet: string | null
}

/** Depth budget + search model preference per mode. Query counts bound each
 *  round; rounds bound the expedition loop. Overridable for tests via env. */
const MODES: Record<ResearchMode, { rounds: number; queries: number; search: string[]; blurb: string }> = {
  recon: { rounds: 1, queries: 1, search: ['sonar', 'sonar-pro'], blurb: 'one fast pass — a cited answer in minutes' },
  brief: { rounds: 1, queries: 3, search: ['sonar-pro', 'sonar'], blurb: 'planned angles, one synthesis — a briefing' },
  expedition: { rounds: 3, queries: 4, search: ['sonar-pro', 'sonar-reasoning-pro', 'sonar'], blurb: 'iterative deep dive — a full report' },
}
const budgetFor = (mode: ResearchMode) => {
  const b = MODES[mode]
  const rounds = Number(process.env.TALARIA_RESEARCH_MAX_ROUNDS ?? 0) || b.rounds
  return { ...b, rounds }
}

export const RESEARCH_MODES: Array<{ mode: ResearchMode; blurb: string }> = (
  Object.entries(MODES) as Array<[ResearchMode, (typeof MODES)[ResearchMode]]>
).map(([mode, b]) => ({ mode, blurb: b.blurb }))

/** A run stuck "running" past this is a crashed pipeline — surfaced as error. */
const STALE_MINUTES = 45

// ── Search stage: sonar through the org gateway, keeping the sources ─────────

interface SearchHit {
  content: string
  sources: Array<{ url: string; title: string | null; snippet: string | null }>
}

/** The search model for a tier: its per-tier MODEL ROLE when assigned (and
 *  still routable) — Perplexity's sonar family maps one-to-one onto the modes
 *  — else the first registered sonar from the mode's preference list.
 *  Research needs a search-capable model on the gateway to exist. */
export async function searchModelFor(mode: ResearchMode): Promise<string | null> {
  const assigned = await resolveRoleModel(`research-${mode}`)
  if (assigned) return assigned
  const ids = new Set((await gatewayModels()).map((m) => m.id))
  for (const want of budgetFor(mode).search) {
    // bare or endpoint-qualified spelling
    if (ids.has(want)) return want
    const qualified = [...ids].find((id) => id.endsWith(`/${want}`))
    if (qualified) return qualified
  }
  return null
}

/** A deep-research-class search model is an agentic researcher in itself
 *  (each call runs its own multi-search sweep) — shrink OUR loop so effort
 *  doesn't multiply: fewer, bigger stages instead of many small ones. */
const isDeepResearchModel = (model: string) => /deep-research/i.test(model)
function adaptBudget(budget: ReturnType<typeof budgetFor>, searchModel: string) {
  if (!isDeepResearchModel(searchModel)) return budget
  return { ...budget, rounds: Math.min(budget.rounds, 2), queries: 1 }
}

/** One search query → sonar's cited answer + its source list. Metered like any
 *  gateway call. Perplexity returns `search_results` (new) / `citations` (old). */
async function searchStage(model: string, query: string, runId: string): Promise<SearchHit> {
  const route = await resolveRoute(model)
  if (!route) throw new Error(`search model "${model}" is not routable`)
  const call = await buildUpstream(route, {
    model,
    stream: false,
    messages: [
      {
        role: 'system',
        content:
          'You are a research search engine. Answer the query with dense, factual, well-sourced findings. Prefer primary sources and recent data. Note dates and numbers precisely.',
      },
      { role: 'user', content: query },
    ],
  })
  const res = await fetchUpstream(call, route)
  if (!res.ok) throw new Error(`search stage ${res.status}: ${(await res.text()).slice(0, 300)}`)
  const j = (await res.json()) as {
    choices?: Array<{ message?: { content?: string } }>
    usage?: { prompt_tokens?: number; completion_tokens?: number }
    search_results?: Array<{ url?: string; title?: string; snippet?: string }>
    citations?: string[]
  }
  if (j.usage) {
    void recordGatewayUsage({
      caller: `research:${runId}`,
      endpoint: route.endpoint,
      upstreamModel: route.upstreamModel,
      promptTokens: j.usage.prompt_tokens ?? 0,
      completionTokens: j.usage.completion_tokens ?? 0,
      estimated: false,
    }).catch(() => {})
  }
  const sources =
    j.search_results?.filter((s) => s.url).map((s) => ({ url: s.url!, title: s.title ?? null, snippet: s.snippet ?? null })) ??
    j.citations?.map((url) => ({ url, title: null, snippet: null })) ??
    []
  return { content: j.choices?.[0]?.message?.content ?? '', sources }
}

// ── Persona stages: the agent's own brain plans and synthesizes ──────────────

/** Non-streaming completion via the agent's persona gateway (its soul, memory,
 *  and domain expertise ride along). Metered like a chat turn. */
async function personaStage(agentModel: string, runId: string, system: string, user: string): Promise<string> {
  const messages = [
    { role: 'system', content: system },
    { role: 'user', content: user },
  ]
  const upstream = await proxyChat({ model: agentModel, messages })
  if (!upstream.ok || !upstream.body) throw new Error(`persona gateway error ${upstream.status}`)
  let text = ''
  let usage: { promptTokens: number; completionTokens: number } | null = null
  for await (const ev of parseAgentStream(upstream.body)) {
    if (ev.type === 'content') text += ev.text
    else if (ev.type === 'usage') usage = ev
  }
  void recordUsage({
    agentModel,
    source: 'research',
    refId: runId,
    tier: null,
    promptTokens: usage?.promptTokens ?? estimateTokens(system.length + user.length),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  }).catch(() => {})
  return text
}

/** Pull a JSON array of strings out of a model reply (fence/prose tolerant). */
function parseQueryList(text: string, max: number): string[] {
  const match = text.match(/\[[\s\S]*?\]/)
  if (match) {
    try {
      const arr = JSON.parse(match[0]) as unknown[]
      return arr.map((q) => String(q).trim()).filter(Boolean).slice(0, max)
    } catch {
      /* fall through to line parsing */
    }
  }
  return text
    .split('\n')
    .map((l) => l.replace(/^[-*\d.\s"']+/, '').replace(/["']$/, '').trim())
    .filter((l) => l.length > 8 && !l.startsWith('#'))
    .slice(0, max)
}

// ── The citation registry ─────────────────────────────────────────────────────

class SourceRegistry {
  private byUrl = new Map<string, { idx: number; title: string | null; snippet: string | null }>()
  add(s: { url: string; title: string | null; snippet: string | null }): number {
    const existing = this.byUrl.get(s.url)
    if (existing) {
      if (!existing.title && s.title) existing.title = s.title
      return existing.idx
    }
    const idx = this.byUrl.size + 1
    this.byUrl.set(s.url, { idx, title: s.title, snippet: s.snippet })
    return idx
  }
  /** Rewrite one search hit's LOCAL [n] markers onto global numbering. */
  renumber(hit: SearchHit): string {
    const map = new Map<number, number>()
    hit.sources.forEach((s, i) => map.set(i + 1, this.add(s)))
    return hit.content.replace(/\[(\d{1,2})\]/g, (m, n) => {
      const g = map.get(Number(n))
      return g ? `[${g}]` : m
    })
  }
  list(): ResearchSource[] {
    return [...this.byUrl.entries()].map(([url, s]) => ({ idx: s.idx, url, title: s.title, snippet: s.snippet }))
  }
  get size(): number {
    return this.byUrl.size
  }
}

// ── Run lifecycle ─────────────────────────────────────────────────────────────

const ROW = `id, owner_user_id as "ownerUserId", requested_by as "requestedBy", agent_model as "agentModel",
  mode, question, status, phase, artifact_id as "artifactId", error, stats,
  created_at as "createdAt", updated_at as "updatedAt", completed_at as "completedAt"`

export async function listResearchRuns(limit = 60): Promise<ResearchRun[]> {
  const sql = await db()
  await sweepStale()
  return (await sql.unsafe(`select ${ROW} from research_runs order by created_at desc limit $1`, [limit])) as unknown as ResearchRun[]
}

export async function getResearchRun(id: string): Promise<{ run: ResearchRun; sources: ResearchSource[] } | null> {
  const sql = await db()
  await sweepStale()
  const rows = (await sql.unsafe(`select ${ROW} from research_runs where id = $1`, [id])) as unknown as ResearchRun[]
  if (!rows[0]) return null
  const sources = (await sql`
    select idx, url, title, snippet from research_sources where run_id = ${id} order by idx asc
  `) as unknown as ResearchSource[]
  return { run: rows[0], sources }
}

async function sweepStale(): Promise<void> {
  const sql = await db()
  await sql`
    update research_runs set status = 'error', error = 'run went stale (app restarted mid-research?)', updated_at = now()
    where status in ('queued', 'running') and updated_at < now() - make_interval(mins => ${STALE_MINUTES})
  `
}

async function setPhase(runId: string, phase: string): Promise<void> {
  const sql = await db()
  await sql`update research_runs set phase = ${phase}, status = 'running', updated_at = now() where id = ${runId}`
}

/** Create a run and kick the pipeline detached. Returns the queued run. */
export async function startResearch(input: {
  question: string
  mode: ResearchMode
  agentModel: string
  ownerUserId: string | null
  requestedBy: string
}): Promise<ResearchRun> {
  const search = await searchModelFor(input.mode)
  if (!search) {
    throw new Error('no search-capable model on the gateway — register a Perplexity sonar model on /models first')
  }
  const sql = await db()
  const rows = (await sql`
    insert into research_runs (owner_user_id, requested_by, agent_model, mode, question)
    values (${input.ownerUserId}, ${input.requestedBy}, ${input.agentModel}, ${input.mode}, ${input.question})
    returning ${sql.unsafe(ROW)}
  `) as unknown as ResearchRun[]
  const run = rows[0]!
  void runResearch(run.id).catch(() => {})
  return run
}

/** The pipeline. Never throws — failures land on the run row. */
async function runResearch(runId: string): Promise<void> {
  const sql = await db()
  try {
    const got = await getResearchRun(runId)
    if (!got || got.run.status === 'error') return
    const { question, mode, agentModel, ownerUserId, requestedBy } = got.run
    const searchModel = (await searchModelFor(mode))!
    const budget = adaptBudget(budgetFor(mode), searchModel)
    const agentLabel = describeAgent(agentModel).label
    const registry = new SourceRegistry()
    const notes: string[] = []
    let queriesRun = 0

    // Round loop: plan queries (persona), search each (sonar), then — in
    // expedition — let the persona name what's still missing and go again.
    let nextQueries: string[] = mode === 'recon' ? [question] : []
    for (let round = 1; round <= budget.rounds; round++) {
      if (nextQueries.length === 0) {
        await setPhase(runId, round === 1 ? 'planning search angles' : `round ${round}: chasing gaps`)
        const planned = await personaStage(
          agentModel,
          runId,
          `You are planning web research in your domain of expertise. Return ONLY a JSON array of ${budget.queries} sharp, non-overlapping search queries (strings) — no prose.`,
          round === 1
            ? `Research question:\n${question}`
            : `Research question:\n${question}\n\nFindings so far:\n${notes.join('\n\n').slice(0, 24_000)}\n\nWhat is still missing, contradictory, or unverified? Give queries that close those gaps. If nothing meaningful remains, return [].`,
        )
        nextQueries = parseQueryList(planned, budget.queries)
        if (nextQueries.length === 0) break // the persona says we're saturated
      }
      for (const q of nextQueries) {
        queriesRun++
        await setPhase(runId, `searching (${queriesRun}): ${q.slice(0, 80)}`)
        try {
          const hit = await searchStage(searchModel, q, runId)
          notes.push(`### Query: ${q}\n${registry.renumber(hit)}`)
        } catch (e) {
          notes.push(`### Query: ${q}\n(search failed: ${(e as Error).message})`)
        }
      }
      nextQueries = []
    }

    if (registry.size === 0) throw new Error('no sources found — search returned nothing citable')

    // Synthesis: the persona writes the document against the global registry.
    await setPhase(runId, 'writing the report')
    const sourceList = registry
      .list()
      .map((s) => `[${s.idx}] ${s.title ?? s.url} — ${s.url}`)
      .join('\n')
    const depth =
      mode === 'recon'
        ? 'a tight, direct answer (a few paragraphs)'
        : mode === 'brief'
          ? 'a structured briefing (~1 page): summary up top, then the key findings under headings'
          : 'a thorough report: executive summary, sections per theme, contradictions and open questions called out'
    const doc = await personaStage(
      agentModel,
      runId,
      `You are writing a research document in your domain. Requirements:
- Start with a "# " title as your very first characters. No lead-in, no code fences.
- EVERY factual claim carries an inline citation marker like [3] referring to the numbered source list you were given. Never invent a number; never cite a source the findings don't support. Uncited claims are defects.
- Write ${depth}.
- Do NOT append a sources section — it is added mechanically from the registry.`,
      `Research question:\n${question}\n\nNumbered sources:\n${sourceList}\n\nFindings (citation markers already on global numbering):\n\n${notes.join('\n\n').slice(0, 80_000)}`,
    )

    // Keep only markers that resolve; append the mechanical Sources section.
    const known = new Set(registry.list().map((s) => s.idx))
    const cleaned = doc
      .trim()
      .replace(/^```[a-z]*\n?|\n?```$/g, '')
      .replace(/\[(\d{1,2})\]/g, (m, n) => (known.has(Number(n)) ? m : ''))
    const cited = new Set([...cleaned.matchAll(/\[(\d{1,2})\]/g)].map((m) => Number(m[1])))
    const sourcesMd = registry
      .list()
      .map((s) => `${s.idx}. [${s.title ?? s.url}](${s.url})${cited.has(s.idx) ? '' : ' *(consulted)*'}`)
      .join('\n')
    const body = `${cleaned}\n\n## Sources\n\n${sourcesMd}\n`
    const title = cleaned.match(/^# (.+)$/m)?.[1]?.trim() ?? `Research — ${question.slice(0, 80)}`

    // The report is a real artifact: org-visible research is the point.
    // Filed under the researching agent's cabinet.
    const artifact = await createArtifact({
      kind: 'doc',
      title,
      createdBy: requestedBy,
      ownerUserId,
      folderId: await agentCategoryFolder(agentLabel, 'Research', requestedBy),
    })
    await saveArtifact(artifact.id, { body, visibility: 'org' }, agentLabel)
    await attachArtifact(artifact.id, { targetType: 'research', targetId: runId }, agentLabel)

    for (const s of registry.list()) {
      await sql`
        insert into research_sources (run_id, idx, url, title, snippet)
        values (${runId}, ${s.idx}, ${s.url}, ${s.title}, ${s.snippet}) on conflict do nothing
      `
    }
    await sql`
      update research_runs set status = 'done', phase = null, artifact_id = ${artifact.id},
        stats = ${sql.json({ queries: queriesRun, sources: registry.size, cited: cited.size })},
        updated_at = now(), completed_at = now()
      where id = ${runId}
    `

    void indexActivity({
      sourceType: 'research',
      sourceId: artifact.id,
      title,
      text: `${title}\n\n${body}`,
      payload: { runId, question, mode },
      href: `/research?r=${runId}`,
    }).catch(() => {})
    if (ownerUserId) {
      void addNotification(ownerUserId, {
        kind: 'research',
        title: `Research ready: ${title.slice(0, 120)}`,
        body: `${agentLabel} finished ${mode === 'recon' ? 'a recon' : mode === 'brief' ? 'a brief' : 'an expedition'} — ${registry.size} sources`,
        href: `/research?r=${runId}`,
      }).catch(() => {})
    }
  } catch (e) {
    await sql`
      update research_runs set status = 'error', phase = null, error = ${(e as Error).message.slice(0, 2000)}, updated_at = now()
      where id = ${runId}
    `
  }
}
