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
import { db } from './db/pg'
import { type ResearchDepth } from './harness/defs/research'
import { generateTitle } from './titler'
import { createConversation } from './conversations'
import { randomUUID } from 'node:crypto'
import { NO_SEARCH_REASON, RESEARCH_MODES, planSearch, researchRun, searchModelFor, type ResearchInput, type SearchPlan } from './runs/defs/research'
import { cancelRun, drive, enqueue } from './runs/run'
// RE-EXPORTED, not re-declared. `planSearch` and its two companions moved into
// the run definition with the pipeline they belong to; every caller — the route,
// the MCP tool, research-plan.test.ts — keeps the import it already had.
export { NO_SEARCH_REASON, RESEARCH_MODES, planSearch, searchModelFor, type SearchPlan }

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
  /** The conversation this run is discussed in. Null until somebody says
   *  something — see `ensureResearchConversation` for why it is on demand. */
  conversationId: string | null
  /** The run this one extends. A follow-up writes into its PARENT's report and
   *  source list rather than minting a second document about one subject; the
   *  child row survives for provenance — who asked what, and what it cost. */
  parentRunId: string | null
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







// ── Persona stages: the agent's own brain plans and synthesizes ──────────────
// Both pin `RunContext.model` to the requesting agent's own persona, which is
// the feature: a marketing agent researches like a marketer. `runHarness` picks
// the fleet transport for it, sends no tools, and guards the reply with an
// honest `Available` for a stream that reports tool names and nothing else.


// ── The citation registry ─────────────────────────────────────────────────────

/** A CITATION MARKER, AND IT IS NOT TWO DIGITS.
 *
 *  `\d{1,2}` was correct for exactly as long as research meant Perplexity: sonar
 *  answers with a handful of pre-ranked sources, so a registry never approached
 *  [99]. Research is model-agnostic now and the tool path is the common one — an
 *  expedition is up to twelve queries against a web-search tool, each returning
 *  a page of results, with every distinct URL numbered. Three figures is
 *  ordinary there.
 *
 *  WHERE IT ACTUALLY BROKE, which is narrower than it first looks and is worth
 *  writing down because the first version of this comment got it wrong. Both
 *  failures are on the REPORT, whose markers are global:
 *
 *    · AN INVENTED [150] SURVIVED. `finishRun` strips markers the registry does
 *      not know; a three-digit one was not matched, so it was neither counted
 *      as dropped nor removed, and it reached the saved document looking exactly
 *      like a real citation.
 *    · THE CITED COUNT UNDERCOUNTED, so a thorough report scored as a thin one,
 *      and `reportProblem` read an all-three-digit report as citing NOTHING.
 *
 *  `SourceRegistry.renumber` was NOT affected and never could be: the markers it
 *  rewrites are LOCAL to one search hit — [1], [2], [3] — and it is the OUTPUT
 *  that carries the global number. Stated because the mutation test that proved
 *  it is easy to read as redundant.
 *
 *  Bounded at three digits rather than left open: `[2024]` in prose is a year,
 *  and matching it would strip dates out of reports. */
const MARKER_RE = /\[(\d{1,3})\]/g

/** Exported for its own test. The renumbering it does is one of the three
 *  places two-digit markers failed silently, and it is not reachable through
 *  `runResearch` without standing up the whole pipeline. */
/** THE SOURCES SECTION THE PIPELINE APPENDS, matched so it can be replaced.
 *
 *  Anchored to a line start and to the END of the document, because a report may
 *  legitimately discuss the word "Sources" in its prose and a loose match would
 *  truncate a report at the first mention of one. */
const SOURCES_SECTION = /\n*^## Sources\s*$[\s\S]*$/m

/** The report's prose, without the mechanical source list. */
export const reportBodyOnly = (body: string): string => body.replace(SOURCES_SECTION, '').trimEnd()

/** EXTEND A REPORT WITH WHAT A FOLLOW-UP FOUND, keeping it one document.
 *
 *  A follow-up used to produce a SECOND report about the same subject, with its
 *  own source numbering, and nothing linking the two. The answer to one question
 *  lived in two places and the reader assembled it.
 *
 *  WHAT THIS HAS TO GET RIGHT, and each one silently corrupts a document if it
 *  does not:
 *
 *    THE OLD PROSE IS UNTOUCHED. Somebody has read it and may have quoted it.
 *    THE SOURCE LIST IS REBUILT, not appended to — it is mechanical output, and
 *      two of them at the bottom of one document is the failure that made the
 *      old `## Sources` regex worth anchoring.
 *    CITED IS RECOMPUTED OVER THE WHOLE DOCUMENT. A source the parent cites and
 *      the follow-up does not must not become "(consulted)" because this pass
 *      only looked at the new section.
 *    THE NEW SECTION SAYS WHAT ASKED FOR IT. A reader coming back a week later
 *      needs to see that the last three paragraphs answer a different question
 *      than the top of the document does. */
export function extendReport(
  parentBody: string,
  addition: { question: string; markdown: string },
  sources: readonly ResearchSource[],
): string {
  const prose = reportBodyOnly(parentBody)
  // The follow-up's own `# Title` is dropped: the document already has one, and
  // a second H1 mid-document reads as a new document to every renderer and
  // every table of contents.
  const section = addition.markdown.trim().replace(/^#\s+.*$/m, '').trim()
  const heading = `## Follow-up: ${addition.question.trim()}`
  const merged = `${prose}\n\n${heading}\n\n${section}`
  const cited = new Set([...merged.matchAll(MARKER_RE)].map((m) => Number(m[1])))
  const list = sources
    .slice()
    .sort((a, b) => a.idx - b.idx)
    .map((s) => `${s.idx}. [${s.title ?? s.url}](${s.url})${cited.has(s.idx) ? '' : ' *(consulted)*'}`)
    .join('\n')
  return `${merged}\n\n## Sources\n\n${list}\n`
}

/** KEEP ONLY THE CITATIONS THAT RESOLVE, and count what was thrown away.
 *
 *  Extracted from the pipeline so it can be tested at all: it is the one place
 *  an invented citation is caught before a human reads the report, and it sat
 *  inline in a function that needs a database, a gateway and an artifact store
 *  to reach.
 *
 *  `dropped` is COUNTED, not merely stripped. Deleting an invented citation is
 *  the right thing to save, but it also made a model that fabricates half its
 *  markers look identical to one that cites perfectly — the exact model-fitness
 *  signal this run is in the best position to report, thrown away by the line
 *  that fixed the symptom. */
export function stripUnknownMarkers(doc: string, knownIdx: readonly number[]): { cleaned: string; dropped: number; cited: Set<number> } {
  const known = new Set(knownIdx)
  const dropped = [...doc.matchAll(MARKER_RE)].filter((m) => !known.has(Number(m[1]))).length
  const cleaned = doc
    .trim()
    .replace(/^```[a-z]*\n?|\n?```$/g, '')
    .replace(MARKER_RE, (m, n) => (known.has(Number(n)) ? m : ''))
  const cited = new Set([...cleaned.matchAll(MARKER_RE)].map((m) => Number(m[1])))
  return { cleaned, dropped, cited }
}

export class SourceRegistry {
  private byUrl = new Map<string, { idx: number; title: string | null; snippet: string | null }>()

  /** SEED FROM A REPORT ALREADY WRITTEN, so a follow-up continues its numbering
   *  instead of starting again at [1].
   *
   *  THIS IS WHAT KEEPS THE OLD TEXT TRUE. Every [n] in the parent's prose
   *  points at a row in its source list; renumbering — or reusing [3] for a new
   *  URL — would silently re-aim citations that a human already read and
   *  believed. So the parent's indices are taken verbatim and new sources
   *  continue from the highest, whatever gaps that leaves. */
  static from(sources: readonly ResearchSource[]): SourceRegistry {
    const reg = new SourceRegistry()
    for (const s of sources) reg.byUrl.set(s.url, { idx: s.idx, title: s.title, snippet: s.snippet })
    return reg
  }
  add(s: { url: string; title: string | null; snippet: string | null }): number {
    const existing = this.byUrl.get(s.url)
    if (existing) {
      if (!existing.title && s.title) existing.title = s.title
      return existing.idx
    }
    // HIGHEST + 1, not size + 1. A seeded registry can carry gaps — a parent
    // whose source [4] was deleted leaves size 3 and a highest of 5 — and
    // `size + 1` would hand [4] to a brand new URL, quietly re-aiming every
    // citation the parent's text makes to [4].
    const idx = Math.max(0, ...[...this.byUrl.values()].map((v) => v.idx)) + 1
    this.byUrl.set(s.url, { idx, title: s.title, snippet: s.snippet })
    return idx
  }
  /** Rewrite one search hit's LOCAL [n] markers onto global numbering.
   *
   *  The hit SHAPE is declared here rather than imported from the run
   *  definition: the pipeline depends on this module, so importing back the
   *  other way would close a cycle for the sake of two fields. */
  renumber(hit: { content: string; sources: Array<{ url: string; title: string | null; snippet: string | null }> }): string {
    const map = new Map<number, number>()
    hit.sources.forEach((s, i) => map.set(i + 1, this.add(s)))
    return hit.content.replace(MARKER_RE, (m, n) => {
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

/** THE PROJECTION. `status` and `phase` are read off the RUN, not off the
 *  research record, and this join is the whole of "one source of truth for
 *  whether a run is alive".
 *
 *  The run row is the authority because it is the thing that is actually being
 *  driven: it is claimed under a lease, it is re-entered by the reclaim sweep,
 *  and its state is the only one that changes when a driver dies. The research
 *  record's own columns are a TERMINAL outcome — written by the run's last step
 *  and by its failure mirror — and they are what this reads when there is no
 *  run at all, which is every row created before this port.
 *
 *  `awaiting` maps to 'running' because `ResearchStatus` has four values, all
 *  of them on the wire and in the client. A parked run is not idle — it is in
 *  somebody's approvals queue with the question on it — and 'running' with the
 *  question in `phase` is the honest four-value spelling of that. A fifth value
 *  is a client change, and it belongs with the runs surface rather than here.
 *  `cancelled` maps to 'error' for the same reason, with the cancel's own
 *  reason carried in `error`.
 *
 *  ONE RUN PER RESEARCH RECORD, AND THE SAME UUID NAMES BOTH — see
 *  `startResearch`. That is what makes this a primary-key join rather than a
 *  scan for the newest run with a matching subject. */
/** THE four-value projection, on its own so it has exactly one spelling. Two
 *  reads need it in different shapes (a full row, and the briefing's filtered
 *  subquery) and a second copy of this CASE is a second answer to "is this run
 *  alive" — which is the whole thing this file is trying not to have. */
const STATUS = `case
    when r.state is null then research_runs.status
    when r.state in ('running', 'awaiting') then 'running'
    when r.state = 'cancelled' then 'error'
    when r.state = 'error' then 'error'
    when r.state = 'done' then 'done'
    else 'queued'
  end`

const ROW = `research_runs.id, research_runs.owner_user_id as "ownerUserId", research_runs.requested_by as "requestedBy",
  research_runs.agent_model as "agentModel", research_runs.mode, research_runs.question, research_runs.title,
  ${STATUS} as status,
  case when r.state in ('queued', 'running', 'awaiting') then nullif(r.phase, '') else research_runs.phase end as phase,
  research_runs.artifact_id as "artifactId",
  coalesce(research_runs.error, r.error) as error,
  research_runs.stats,
  research_runs.created_at as "createdAt",
  greatest(research_runs.updated_at, coalesce(r.updated_at, research_runs.updated_at)) as "updatedAt",
  research_runs.completed_at as "completedAt"`

/** Always joined, never selected from alone: the projection above needs `r`. */
const FROM = `from research_runs left join runs r on r.id = research_runs.id and r.kind = 'research'`

/** Runs a viewer may see: their own, ones shared with them, and org runs
 *  (no owner — general agents researching for the org). null viewer (a
 *  general agent) sees org runs only. */
export async function listResearchRuns(viewerUserId: string | null, limit = 60): Promise<ResearchRun[]> {
  const sql = await db()
  if (viewerUserId === null) {
    return (await sql.unsafe(
      `select ${ROW} ${FROM} where research_runs.owner_user_id is null order by research_runs.created_at desc limit $1`,
      [limit],
    )) as unknown as ResearchRun[]
  }
  return (await sql.unsafe(
    `select ${ROW} ${FROM}
     where research_runs.owner_user_id is null or research_runs.owner_user_id = $1
        or exists(select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = $1)
     order by research_runs.created_at desc limit $2`,
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
/** Research this person should be told about in an ambient briefing: what is in
 *  flight, plus anything that failed in the last week.
 *
 *  IT EXISTS SO THE PROJECTION IS SPELLED ONCE. `briefing.ts` asked
 *  `research_runs` directly — `where status in ('queued','running')` — and under
 *  the port that column is a TERMINAL outcome: a run that is happily searching
 *  right now still reads 'queued', and a run a driver GAVE UP on from the
 *  outside (attempts spent, a step over its budget, a cancel) reads 'queued'
 *  for ever. The briefing would have told somebody "research queued: ..." every
 *  morning about a run that stopped weeks ago, which is the same class of bug as
 *  the sentence this project deleted: a stale column narrating a run nobody is
 *  driving.
 *
 *  Same join, same CASE, same file as every other read — see `ROW`. */
/** The in-flight run for a question, if there is one — the double-click guard.
 *
 *  IT ASKS THE RUN, which is the point of it existing. /api/research does this
 *  today with `where question = $1 and status in ('queued','running')` in raw
 *  SQL against the research record, and that column is now a TERMINAL outcome
 *  rather than a live state: it is written when a run finishes or when a step
 *  records its own failure, and NOT when a driver gives up on a run from the
 *  outside (attempts spent, a step over its budget, a cancel). Left on the raw
 *  column, one of those would make a question unaskable for ever — the record
 *  would still read 'queued' with nothing driving it.
 *
 *  `awaiting` counts as in flight: a run parked on "search again?" is very much
 *  the same question, still open, waiting on the person about to ask it twice. */
export async function activeResearchOn(question: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select research_runs.id from research_runs
    left join runs r on r.id = research_runs.id and r.kind = 'research'
    where research_runs.question = ${question}
      and case when r.state is null then research_runs.status in ('queued', 'running')
               else r.state in ('queued', 'running', 'awaiting') end
    limit 1
  `) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
}

export async function briefableResearch(userId: string, limit = 10): Promise<Array<{ id: string; question: string; status: ResearchStatus }>> {
  const sql = await db()
  return (await sql.unsafe(
    `select id, question, status from (
       select research_runs.id,
              coalesce(research_runs.title, research_runs.question) as question,
              ${STATUS} as status,
              research_runs.created_at
       ${FROM}
       where research_runs.owner_user_id = $1
          or exists(select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = $1)
     ) s
     where s.status in ('queued', 'running')
        or (s.status = 'error' and s.created_at > now() - interval '7 days')
     order by s.created_at desc limit $2`,
    [userId, limit],
  )) as unknown as Array<{ id: string; question: string; status: ResearchStatus }>
}

/** Throw the run away: STOP IT FIRST, then delete the record.
 *
 *  DELETE /api/research/:id used to be one `delete from research_runs`, which
 *  under the port stops nothing — the work lives on the `runs` row, and the
 *  driver holding it goes on planning, searching and synthesizing a report for a
 *  record that no longer exists. The run's own steps do check `rowExists` and
 *  stop, so nothing was permanently wrong; the cost was up to one whole step
 *  (eleven minutes, and a billed search inside it) spent on work somebody had
 *  just thrown away.
 *
 *  THE ORDER IS THE POINT. `cancelRun` is a compare-and-set with no lease
 *  predicate, so it lands from any instance and every subsequent write by the
 *  driver that owns the run is refused — the stop is real before the row goes.
 *  The other order leaves a window where the record is gone and the run is still
 *  `running`, which is the state a reclaim sweep would happily re-enter.
 *
 *  The report artifact SURVIVES, unchanged: deleting a run clears the queue
 *  entry, not the knowledge. */
export async function deleteResearchRun(runId: string): Promise<void> {
  // Not conditional on the result: `missing` (a row from before this port) and
  // `terminal` (a finished run) are both perfectly normal here, and neither is
  // a reason to leave the record behind.
  await cancelRun({ runId, reason: 'the research run was deleted' }).catch((e) =>
    console.error('[research] could not cancel', runId, 'before deleting it:', e),
  )
  const sql = await db()
  await sql`delete from research_runs where id = ${runId}`
}

export async function researchArtifactFor(runId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select artifact_id as id from artifact_links where target_type = 'research' and target_id = ${runId} limit 1
  `) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
}

export async function getResearchRun(id: string): Promise<{ run: ResearchRun; sources: ResearchSource[] } | null> {
  const sql = await db()
  // NO SWEEP HERE, AND NONE IN THE LIST EITHER. Both reads used to call
  // `sweepStale()` first, so OPENING THE RESEARCH PAGE was what failed a run
  // that had outlived a deploy: the person who asked the question triggered the
  // sentence telling them it had gone stale. A restart is not a failure, no
  // read of this table decides anything about a run's health any more, and the
  // event that used to produce an epitaph now produces a resume — see
  // `runs/reclaim.ts`, which is where that sweep went.
  const rows = (await sql.unsafe(`select ${ROW} ${FROM} where research_runs.id = $1`, [id])) as unknown as ResearchRun[]
  const run = rows[0]
  if (!run) return null
  const sources = (await sql`
    select idx, url, title, snippet from research_sources where run_id = ${id} order by idx asc
  `) as unknown as ResearchSource[]
  return { run, sources }
}



/** Create the run and start it. Returns the queued research record.
 *
 *  THE ORDER IS THE DURABILITY. The `runs` row goes in FIRST, because it is the
 *  record that survives: it carries the question, the mode, the agent and the
 *  owner on its `input`, so a process that dies between the two inserts leaves
 *  something the reclaim sweep can pick up — and the run's first step writes
 *  the research record from that input when it is not there. The other order
 *  leaves the opposite: a research record nobody is driving, nothing to
 *  reclaim, and no stale sweep left to notice it.
 *
 *  ONE RUN PER RESEARCH RECORD, AND ONE UUID NAMING BOTH. `enqueue`
 *  deduplicates nothing above the row (risk 6 in runs/define.ts), so the id is
 *  passed in rather than generated inside it: a caller that retried this
 *  function with the same id would collide on the primary key instead of
 *  starting a second run doing the same work. The duplicate-QUESTION check that
 *  stops a double click is /api/research's, and it is unchanged.
 *
 *  SIGNATURE UNCHANGED, including the up-front refusal when no sonar model is
 *  registered: that is a 400 the caller shows in the form, and turning it into
 *  a run that fails a second later would be a worse answer to the same
 *  question. The run's first step re-checks it, for the resume case. */
export async function startResearch(input: {
  question: string
  mode: ResearchMode
  agentModel: string
  ownerUserId: string | null
  requestedBy: string
  /** Set when this is a follow-up asked from a report's own conversation. */
  parentRunId?: string | null
}): Promise<ResearchRun> {
  const search = await searchModelFor(input.mode)
  if (!search) {
    throw new Error('no search-capable model on the gateway — register a Perplexity sonar model on /models first')
  }
  const id = randomUUID()
  const runInput: ResearchInput = {
    question: input.question,
    mode: input.mode,
    agentModel: input.agentModel,
    ownerUserId: input.ownerUserId,
    requestedBy: input.requestedBy,
    parentRunId: input.parentRunId ?? null,
  }
  // `start: false`, then the domain row, then drive. The detached drive is a
  // nicety and never the guarantee (see `EnqueueOptions`) — but starting it
  // here would have the first step racing this function to write the research
  // record, and the loser of that race is the caller: `ensureRow` is `on
  // conflict do nothing`, so the insert below would return no row to render.
  await enqueue(researchRun, runInput, {
    id,
    ownerUserId: input.ownerUserId,
    subjectType: 'research',
    subjectId: id,
    phase: 'queued',
    start: false,
  })

  const sql = await db()
  await sql`
    insert into research_runs (id, owner_user_id, requested_by, agent_model, mode, question, parent_run_id)
    values (${id}, ${input.ownerUserId}, ${input.requestedBy}, ${input.agentModel}, ${input.mode}, ${input.question}, ${input.parentRunId ?? null})
    on conflict (id) do nothing
  `
  // Read back through the projection rather than `returning`, so the row this
  // hands to the caller is spelled by the same join every other read uses. One
  // indexed read at the start of a run that is about to make model calls.
  const created = await getResearchRun(id)
  if (!created) throw new Error('could not create the research run')

  // The Titler names the run from its question — fire-and-forget, the list
  // shows the raw question until the title lands. Deliberately NOT a step: its
  // only output is one idempotent column write that nothing waits on, and a
  // step would put another billable call in the run's critical path (and in the
  // set of calls a reclaim repeats) to save a title.
  void generateTitle('research', input.question)
    .then(async (t) => {
      if (t) await sql`update research_runs set title = ${t} where id = ${id}`
    })
    .catch(() => {})

  // Detached, exactly as `void runResearch(id)` was — and the difference is
  // everything behind it: if this process never gets to the work, or dies in
  // the middle of it, the row is `queued` or `running` with a stale lease and
  // the reclaim sweep takes it from the last checkpoint.
  void drive(id).catch((e) => console.error('[research] detached drive of', id, 'threw:', e))
  return created.run
}


// ── The conversation a run is discussed in ───────────────────────────────────
//
// A research run used to be a one-shot: ask, wait, read. The only thing anyone
// could do afterwards was read it again, and the two colleagues it was shared
// with had nowhere to say "dig into the second point" or "this source is a
// vendor blog". A view whose whole content is a document does not need to be a
// view.
//
// So a run gets a conversation of its own, exactly the shape a plan has —
// several people, one agent, one document that grows beside the talk.

/** WHAT THE AGENT IS FOR IN THIS ROOM.
 *
 *  It already knows the report: the findings, the numbered sources and what each
 *  one supports. Most turns here are that — answering from what was found, which
 *  costs one call and no searching.
 *
 *  THE RULE THAT MATTERS is the one about not answering past the evidence. This
 *  is the surface where a confident sentence is most likely to be believed,
 *  because everything around it carries a citation: the report is cited, the
 *  sources are listed, and a teammate skim-reading has no way to tell which
 *  sentence came from a source and which came from the model. So an answer the
 *  report does not support has to say so and offer to go and find out — which is
 *  a real offer, because commissioning more research is a tool it holds.
 *
 *  IT DOES NOT SEARCH BY REFLEX. A follow-up run costs minutes and money, and a
 *  question the report already answers is not a reason to spend either. */
export const RESEARCH_MODE_PROMPT = `This is a conversation ABOUT a research report you produced, on the Research surface. Several teammates may be in it; the report and its numbered sources sit beside the chat.

Answer from the report and its sources first. Cite the same [n] markers the report uses so anyone can check you, and never state something as established that the sources do not support — this is a surface where everything looks cited, so an uncited claim of yours will be read as a finding.

When the answer genuinely is not in what was found, say so plainly and offer to look into it. If a teammate asks you to dig further, commission follow-up research with the research tool rather than answering from memory; its findings are added to the same report, so the document stays the one place the answer lives.

Do not re-research something the report already covers — say what it says and point at the section.`

/** The conversation for a run, created on first use.
 *
 *  ON DEMAND rather than at run creation, for a reason worth stating: most runs
 *  are read once and never discussed, and a conversation row per run would make
 *  the chat list a list of things nobody said anything about. The first message
 *  is what makes it exist.
 *
 *  Owned by the run's owner and pinned to the agent that DID the research, so
 *  the teammate answering questions about the report is the one that wrote it. */
export async function ensureResearchConversation(runId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select conversation_id as "conversationId", owner_user_id as "ownerUserId", agent_model as "agentModel", question, title
      from research_runs where id = ${runId}
  `) as unknown as Array<{ conversationId: string | null; ownerUserId: string | null; agentModel: string; question: string; title: string | null }>
  const run = rows[0]
  if (!run) return null
  if (run.conversationId) return run.conversationId
  // A run started by an agent with no human owner has nobody to own the
  // conversation. It is still readable; it just cannot be talked in.
  if (!run.ownerUserId) return null

  const id = await createConversation(run.ownerUserId, run.agentModel, run.title ?? run.question.slice(0, 80), 'research', null)
  await sql`update research_runs set conversation_id = ${id} where id = ${runId} and conversation_id is null`
  // Re-read rather than trusting the write: two clients opening the same run at
  // once both get here, and the loser must return the winner's conversation
  // instead of a second one nobody is reading.
  const after = (await sql`select conversation_id as "conversationId" from research_runs where id = ${runId}`) as unknown as Array<{
    conversationId: string | null
  }>
  return after[0]?.conversationId ?? id
}

/** The run a conversation belongs to, for the surfaces that start from the chat
 *  side rather than the report side. */
export async function researchRunForConversation(conversationId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select id from research_runs where conversation_id = ${conversationId}`) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
}
