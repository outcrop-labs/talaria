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
//
// AUDIT 1.5 / 1.6 — WHAT THE HARNESS PORT CHANGED HERE. All three model calls
// (search, plan, synthesize) went out by hand and none of them was guarded, on
// the one path in the product whose defining failure mode is a fabricated
// citation. They are declared in `harness/defs/research.ts` now and run through
// `runHarness`, which owns the model, the capability floor, the parse, the
// repair turn, the guard pass and the ledger. ONE thing stays HERE on purpose
// and it is commented where it happens: the line-based salvage of a query list
// (the tolerance the old extractor had, kept as a declared salvage rather than
// a silent one).
//
// The `ungrounded_ref` pass used to be the second exception — a `guardSynthesis`
// function in this file, running one rule over a hand-built tool record,
// because the runner had no way for a harness to declare what it was grounded
// against. `HarnessDefinition.ground` closed that, so the rule is in
// `researchSynthesisHarness`'s own guard list and this file counts the findings
// the run already reported. Nothing here talks to guardrails.ts any more.
import { agentCategoryFolder, attachArtifact, createArtifact, saveArtifact } from './artifacts'
import { setEditors } from './kb-perms'
import { db } from './db/pg'
import { describeAgent } from './gateway'
import {
  clampQueries,
  queriesFromLines,
  researchQueriesHarness,
  researchSearchHarness,
  researchSynthesisHarness,
  searchTransport,
  toolSearchTransport,
  type ResearchDepth,
  type SearchSource,
} from './harness/defs/research'
import { capabilityKeysFor, runHarness } from './harness/run'
import { reachFor, type Reach } from './capability-reach'
import { gatewayModels } from './llm-gateway'
import { resolveRoleModel } from './model-roles'
import { addNotification } from './notifications'
import { indexActivity, indexPersonal } from './retrieval/sources'
import { generateTitle } from './titler'

/** Unchanged as a public name and as a type — `ResearchDepth` is declared in
 *  the harness definition because that module cannot import this one, and this
 *  alias keeps every existing importer of `ResearchMode` working. */
export type ResearchMode = ResearchDepth
export type ResearchStatus = 'queued' | 'running' | 'done' | 'error'

export interface ResearchRun {
  id: string
  ownerUserId: string | null
  requestedBy: string
  agentModel: string
  mode: ResearchMode
  question: string
  title: string | null
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
  sources: SearchSource[]
}

/** The search model for a tier: its per-tier MODEL ROLE when assigned (and
 *  still routable) — Perplexity's sonar family maps one-to-one onto the modes
 *  — else the first registered sonar from the mode's preference list.
 *  Research needs a search-capable model on the gateway to exist.
 *
 *  AUDIT 1.6: `resolveRoleModel` validates that the assignment still ROUTES and
 *  nothing else, so what comes back here may have no web search at all — the
 *  auto path's careful sonar preference is bypassed the moment an admin picks
 *  something. The RUNTIME defense is one layer down: `researchSearchHarness`
 *  declares `requires: ['search']` with `refuseBelow: true`, so a model
 *  positively known not to search refuses the stage instead of hallucinating a
 *  cited brief. This function stays permissive on purpose — unknown is not
 *  missing, and a fresh self-host has probed nothing. */
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
 *  doesn't multiply: fewer, bigger stages instead of many small ones.
 *
 *  AUDIT NOTE, left as-is deliberately: this is capability reasoning done by
 *  REGEX ON A MODEL NAME, which is the shape `harness/capability.ts` exists to
 *  replace. It is not converted here because "runs its own multi-search sweep"
 *  is a genuinely new capability, not one of the nine that module declares, and
 *  inventing a capability id that only one regex writes is worse than leaving
 *  the regex where a reader can see it. The right end state is a
 *  `deep-research` capability that a probe or a catalog declares, at which
 *  point this becomes a `widen` on the search harness — a capable model
 *  earning the fewer-bigger-stages loop — rather than a name match. Until then
 *  the regex is honest about being a heuristic. */
const isDeepResearchModel = (model: string) => /deep-research/i.test(model)
function adaptBudget(budget: ReturnType<typeof budgetFor>, searchModel: string) {
  if (!isDeepResearchModel(searchModel)) return budget
  return { ...budget, rounds: Math.min(budget.rounds, 2), queries: 1 }
}

/** One search query → the search model's cited answer + its source list.
 *
 *  Signature unchanged, and so is what a THROW means: the round loop catches
 *  per query and records "(search failed: ...)" against that angle, so one dead
 *  query costs one angle rather than the run, and a run where every query died
 *  ends with no citable sources — which the pipeline already treats as a hard
 *  error.
 *
 *  The capability floor is what keeps it honest. The harness declares `requires:
 *  ['search']` and refuses below it, so a model an admin assigned to a
 *  `research-*` role that CANNOT REACH search fails loudly here instead of
 *  answering fluently from training data and handing the synthesis stage an
 *  uncited brief to write up (audit 1.6).
 *
 *  TWO WAYS TO REACH IT, and this stage picks between them:
 *
 *    natively   the model browses. Sources come off the provider's own
 *               `search_results`/`citations` fields — `searchTransport`.
 *    by tool    the model does not browse, but this org has registered a
 *               web-search MCP server and the model calls tools. Sources come
 *               off the tool's payload — `toolSearchTransport`.
 *
 *  Native wins when both are available: it is one round trip rather than three,
 *  the provider has already done the ranking, and it is what an admin who
 *  assigned a sonar model asked for. The stage's OUTPUT is identical either way
 *  — prose findings plus a source list — which is what makes the tool path a
 *  real alternative rather than a downgrade, and why nothing downstream branches
 *  on which one ran. */
/** WHICH SEARCH PATH THIS RUN TAKES, asked ONCE per run.
 *
 *  Asked from `capability-reach.ts` — the same module the floor asks — so the
 *  stage and the refusal that guards it can never disagree about whether search
 *  is reachable. Two spellings of that question is how a stage comes to run a
 *  transport the floor was about to refuse.
 *
 *  ONCE PER RUN, not once per query: the answer is a property of the model and
 *  the org's registry, neither of which changes inside a run, and an expedition
 *  issues up to twelve queries. A registry read per query would be eleven reads
 *  for one answer.
 *
 *  A FAILURE HERE CHOOSES THE NATIVE PATH, which is the safe default in the
 *  precise sense that matters: it is the path the floor understands. If search
 *  is genuinely unreachable, the native transport runs, the floor refuses it,
 *  and the run reports the real problem — rather than this function inventing a
 *  tool path out of a failed lookup. */
async function searchPathFor(model: string): Promise<{ server: string; tool: string } | null> {
  const keys = await capabilityKeysFor(model).catch((): string[] => [])
  const reach = await reachFor(keys, ['search']).catch((): Record<string, Reach> => ({}))
  return reach['search']?.via === 'tool' ? reach['search'].supplier : null
}

async function searchStage(model: string, query: string, runId: string, viaTool: { server: string; tool: string } | null): Promise<SearchHit> {
  const sources: SearchSource[] = []
  const run = await runHarness(
    researchSearchHarness,
    { query },
    {
      caller: `research:${runId}`,
      model,
      deps: { transport: viaTool ? toolSearchTransport(runId, sources, viaTool) : searchTransport(runId, sources) },
    },
  )
  // `researchSearchHarness` declares `onFailure: 'throw'` and `runHarness` now
  // honors it on every path that fails to produce a value — the transport and
  // the pre-call ones included — so this is a TYPE narrowing rather than a
  // second copy of the policy. `HarnessResult.value` is `O | null` on every
  // result alike, and the compiler cannot see the guarantee the runner makes.
  if (run.value === null) throw new Error(run.error ?? `search stage produced nothing on "${model}"`)
  return { content: run.value, sources }
}

// ── Persona stages: the agent's own brain plans and synthesizes ──────────────
// Both pin `RunContext.model` to the requesting agent's own persona, which is
// the feature: a marketing agent researches like a marketer. `runHarness` picks
// the fleet transport for it, sends no tools, and guards the reply with an
// honest `Available` for a stream that reports tool names and nothing else.

/** This round's search queries, or an empty list when the persona says the
 *  question is saturated. Never throws — a failed plan is one lost round, and
 *  the loop already treats an empty list as "stop".
 *
 *  THE LINE-BASED SALVAGE, which used to live inside the extractor. On a small
 *  model a numbered list is likelier than a JSON array, and the old
 *  `parseQueryList` quietly read one — so a model that never once produced the
 *  declared contract was indistinguishable from a model that always did. The
 *  tolerance is kept and the silence is not: the harness records the contract
 *  failure on its `harness_runs` row, and the salvage runs afterwards, here,
 *  where it is visible.
 *
 *  It runs ONLY on a reply the guard found nothing in. These strings are sent
 *  onward to a third-party search model, and guardrails.ts's cardinal invariant
 *  is that flagged content never re-enters a model's context — a rule the
 *  runner honors for its own repair turn and which a salvage path must not
 *  quietly route around. */
async function planQueries(agentModel: string, runId: string, question: string, findingsSoFar: string[], max: number): Promise<string[]> {
  const run = await runHarness(
    researchQueriesHarness,
    { question, max, findingsSoFar },
    // Both persona stages of a run carry the same attribution — see the
    // synthesis call for why `source: 'research'` exists at all. An expedition
    // plans up to three times, so leaving this off would make the planning half
    // of a run's cost the half nobody could find.
    { caller: `research:${runId}`, model: agentModel, ledger: { source: 'research', refId: runId } },
  )
  if (run.value !== null) return clampQueries(run.value, max)
  if (run.raw && run.findings.length === 0) return clampQueries(queriesFromLines(run.raw), max)
  return []
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
  mode, question, title, status, phase, artifact_id as "artifactId", error, stats,
  created_at as "createdAt", updated_at as "updatedAt", completed_at as "completedAt"`

/** Runs a viewer may see: their own, ones shared with them, and org runs
 *  (no owner — general agents researching for the org). null viewer (a
 *  general agent) sees org runs only. */
export async function listResearchRuns(viewerUserId: string | null, limit = 60): Promise<ResearchRun[]> {
  const sql = await db()
  await sweepStale()
  if (viewerUserId === null) {
    return (await sql.unsafe(`select ${ROW} from research_runs where owner_user_id is null order by created_at desc limit $1`, [limit])) as unknown as ResearchRun[]
  }
  return (await sql.unsafe(
    `select ${ROW} from research_runs
     where owner_user_id is null or owner_user_id = $1
        or exists(select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = $1)
     order by created_at desc limit $2`,
    [viewerUserId, limit],
  )) as unknown as ResearchRun[]
}

/** The viewer's standing on a run: owner, member (incl. org runs), or none. */
export async function researchRole(viewerUserId: string | null, runId: string): Promise<'owner' | 'member' | null> {
  const sql = await db()
  const rows = (await sql`
    select owner_user_id as "ownerUserId",
           exists(select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = ${viewerUserId}) as member
    from research_runs where id = ${runId}
  `) as unknown as Array<{ ownerUserId: string | null; member: boolean }>
  const r = rows[0]
  if (!r) return null
  if (r.ownerUserId === null) return 'member' // org run — anyone signed in
  if (viewerUserId && r.ownerUserId === viewerUserId) return 'owner'
  return r.member ? 'member' : null
}

export interface ResearchMember {
  userId: string
  name: string | null
  email: string | null
  role: 'owner' | 'collaborator'
}

export async function listResearchMembers(runId: string): Promise<ResearchMember[]> {
  const sql = await db()
  return (await sql`
    select u.id as "userId", u.name, u.email, 'owner' as role
    from research_runs r join users u on u.id = r.owner_user_id where r.id = ${runId}
    union all
    select u.id as "userId", u.name, u.email, 'collaborator' as role
    from research_members rm join users u on u.id = rm.user_id where rm.run_id = ${runId}
  `) as unknown as ResearchMember[]
}

export async function addResearchMember(runId: string, userId: string): Promise<void> {
  const sql = await db()
  await sql`insert into research_members (run_id, user_id) values (${runId}, ${userId}) on conflict do nothing`
}

export async function removeResearchMember(runId: string, userId: string): Promise<void> {
  const sql = await db()
  await sql`delete from research_members where run_id = ${runId} and user_id = ${userId}`
}

/** The run's report artifact (via artifact_links), if it exists yet. */
export async function researchArtifactFor(runId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select artifact_id as id from artifact_links where target_type = 'research' and target_id = ${runId} limit 1
  `) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
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
  // The Titler names the run from its question — fire-and-forget, the list
  // shows the raw question until the title lands.
  void generateTitle('research', input.question)
    .then(async (t) => {
      if (t) await sql`update research_runs set title = ${t} where id = ${run.id}`
    })
    .catch(() => {})
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
    // Which search path this run takes — resolved once, before the first query.
    const searchTool = await searchPathFor(searchModel)
    const agentLabel = describeAgent(agentModel).label
    const registry = new SourceRegistry()
    const notes: string[] = []
    let queriesRun = 0
    let searchFailed = false

    // Round loop: plan queries (persona), search each (sonar), then — in
    // expedition — let the persona name what's still missing and go again.
    let nextQueries: string[] = mode === 'recon' ? [question] : []
    for (let round = 1; round <= budget.rounds; round++) {
      if (nextQueries.length === 0) {
        await setPhase(runId, round === 1 ? 'planning search angles' : `round ${round}: chasing gaps`)
        nextQueries = await planQueries(agentModel, runId, question, round === 1 ? [] : notes, budget.queries)
        if (nextQueries.length === 0) break // the persona says we're saturated
      }
      for (const q of nextQueries) {
        queriesRun++
        await setPhase(runId, `searching (${queriesRun}): ${q.slice(0, 80)}`)
        try {
          const hit = await searchStage(searchModel, q, runId, searchTool)
          notes.push(`### Query: ${q}\n${registry.renumber(hit)}`)
        } catch (e) {
          searchFailed = true
          notes.push(`### Query: ${q}\n(search failed: ${(e as Error).message})`)
        }
      }
      nextQueries = []
    }

    if (registry.size === 0) throw new Error('no sources found — search returned nothing citable')

    // Synthesis: the persona writes the document against the global registry.
    // Throws on an unusable reply rather than saving an empty report — the
    // searches are already paid for and this document is the run's only
    // deliverable, so "keep what you had" is not an option here.
    //
    // THE GROUNDING PASS IS INSIDE THIS CALL. `searchFailed` and the findings
    // are on the INPUT because `researchSynthesisHarness` declares `ground`
    // (see its comment): the search hits are this turn's tool results, so
    // `ungrounded_ref` — the definitive research failure mode — runs on the
    // report, from the runner, with one findings row per fabricated link. This
    // adapter used to run that rule itself in a `guardSynthesis` function; the
    // rule moved into the harness's declared list and the function is gone.
    await setPhase(runId, 'writing the report')
    const synthesis = await runHarness(
      researchSynthesisHarness,
      { question, mode, sources: registry.list().map((s) => ({ idx: s.idx, url: s.url, title: s.title })), findings: notes, searchFailed },
      // `source: 'research'` + the run id is what makes a run's COST answerable.
      // The persona stages used to meter as an ownerless chat turn, so research
      // spend was indistinguishable from every other persona call in
      // `usage_events`. The search stages meter themselves through the gateway.
      { caller: `research:${runId}`, model: agentModel, ledger: { source: 'research', refId: runId } },
    )
    // A TYPE NARROWING, not a policy — `researchSynthesisHarness` declares
    // `onFailure: 'throw'` and the runner honors it on every path that fails to
    // produce a value. It used to be a policy, restated here by hand, because
    // 'throw' covered a CONTRACT failure and nothing else: a persona gateway
    // answering 502 mid-synthesis arrived as `value: null`, and the `?? ''` this
    // replaced saved an artifact containing only the Sources list, marked the
    // run `done`, indexed the empty report and notified the requester — with the
    // gateway's own sentence dropped on the floor.
    if (synthesis.value === null) throw new Error(synthesis.error ?? 'the report came back empty')
    const doc = synthesis.value

    // Keep only markers that resolve; append the mechanical Sources section.
    //
    // `dropped` is counted rather than only stripped. Deleting an invented
    // citation is the right thing to save, but it also made a model that
    // fabricates half its markers look identical to one that cites perfectly —
    // the exact model-fitness signal this run is in the best position to
    // report, thrown away by the line that fixed the symptom.
    const known = new Set(registry.list().map((s) => s.idx))
    const dropped = [...doc.matchAll(/\[(\d{1,2})\]/g)].filter((m) => !known.has(Number(m[1]))).length
    const cleaned = doc
      .trim()
      .replace(/^```[a-z]*\n?|\n?```$/g, '')
      .replace(/\[(\d{1,2})\]/g, (m, n) => (known.has(Number(n)) ? m : ''))
    const cited = new Set([...cleaned.matchAll(/\[(\d{1,2})\]/g)].map((m) => Number(m[1])))

    // The stat, read off the run rather than off a second guard pass. Filtered
    // to `ungrounded_ref` because `synthesis.findings` also carries the
    // `secret_leak` / `pii_leak` hits, and this number has one meaning on the
    // research surface: how many links this model asserted that no search
    // result backs. `dropped` above is its deterministic sibling — invented [n]
    // markers — and the two are counted separately on purpose.
    //
    // The guard ran on the RAW reply, not on `cleaned`. That is not a widening:
    // `ungrounded_ref` looks for URLs on policed hosts and for UUIDs, and
    // `cleaned` only strips code fences and unresolvable [n] markers, neither of
    // which can be either.
    const ungrounded = synthesis.findings.filter((f) => f.check === 'ungrounded_ref').length
    const sourcesMd = registry
      .list()
      .map((s) => `${s.idx}. [${s.title ?? s.url}](${s.url})${cited.has(s.idx) ? '' : ' *(consulted)*'}`)
      .join('\n')
    const body = `${cleaned}\n\n## Sources\n\n${sourcesMd}\n`
    const title = cleaned.match(/^# (.+)$/m)?.[1]?.trim() ?? `Research — ${question.slice(0, 80)}`

    // The report is a real artifact, filed under the researching agent's
    // cabinet. Ownership decides reach: an ORG run (no owner) publishes
    // org-visible; a user's run stays PRIVATE to them, with members granted
    // editor on the doc — sharing the run is the only way anyone else sees it.
    const artifact = await createArtifact({
      kind: 'doc',
      title,
      createdBy: requestedBy,
      ownerUserId,
      folderId: await agentCategoryFolder(agentLabel, 'Research', requestedBy),
    })
    await saveArtifact(artifact.id, { body, visibility: ownerUserId ? 'private' : 'org' }, agentLabel)
    if (ownerUserId) {
      const members = (await sql`select user_id as id from research_members where run_id = ${runId}`) as unknown as Array<{ id: string }>
      if (members.length) {
        await setEditors('artifact', artifact.id, members.map((m) => ({ principalType: 'user' as const, principalId: m.id, role: 'editor' as const }))).catch(() => {})
      }
    }
    await attachArtifact(artifact.id, { targetType: 'research', targetId: runId }, agentLabel)

    for (const s of registry.list()) {
      await sql`
        insert into research_sources (run_id, idx, url, title, snippet)
        values (${runId}, ${s.idx}, ${s.url}, ${s.title}, ${s.snippet}) on conflict do nothing
      `
    }
    await sql`
      update research_runs set status = 'done', phase = null, artifact_id = ${artifact.id},
        stats = ${sql.json({ queries: queriesRun, sources: registry.size, cited: cited.size, dropped, ungrounded })},
        updated_at = now(), completed_at = now()
      where id = ${runId}
    `

    // Placement follows ownership: a personal run's report goes to the owner's
    // private brain (them + their assistant); an org-wide run's report goes to
    // the ambient index, marked orgWide so scopes actually match it.
    const reportDoc = {
      sourceType: 'research',
      sourceId: artifact.id,
      title,
      text: `${title}\n\n${body}`,
      payload: ownerUserId ? { runId, question, mode } : { runId, question, mode, orgWide: true },
      href: `/research?r=${runId}`,
    }
    if (ownerUserId) void indexPersonal(ownerUserId, reportDoc).catch(() => {})
    else void indexActivity(reportDoc).catch(() => {})
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
