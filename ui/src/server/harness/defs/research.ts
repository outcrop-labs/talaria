// The three research harnesses: plan the queries, run a search, write the report.
//
// WHY THIS FILE EXISTS (audit 1.5 and 1.6, and this is where they meet)
//   `server/research.ts` reached a model THREE ways and none of them was
//   guarded:
//
//     searchStage    buildUpstream + fetchUpstream against a sonar model, whose
//                    `search_results` / `citations` are the run's whole product
//     personaStage   proxyChat against the requesting agent's own persona, used
//                    twice - once to plan queries, once to write the document
//     parseQueryList a FOURTH structured-output extractor: a non-greedy
//                    `/\[[\s\S]*?\]/` plus a line-based fallback
//
//   The extractor is the one place in the tree that half-learned the lesson
//   `harness/json.ts` now states in full - a non-greedy match plus a tolerant
//   fallback is closer to right than the four "first brace to last brace"
//   copies elsewhere - and it still had two ways to go wrong that the balanced
//   scanner does not: `/\[[\s\S]*?\]/` matches a `[2]` CITATION MARKER in the
//   model's preamble and reads it as the query list, and a numbered list with
//   no JSON in it at all took the line fallback silently, so nothing anywhere
//   recorded that the model had failed the contract. Both are preserved in
//   spirit and fixed in fact below: the schema REJECTS a decorative `[2]` so
//   the scanner walks on to the real candidate, and the line fallback lives in
//   the adapter where it is a declared salvage of a run the fitness data has
//   already recorded as a contract failure.
//
//   The guard gap is the sharper half. A research report is persisted as a doc
//   artifact, shared with the run's members, and INDEXED INTO THE BRAIN, so
//   every future chat and plan can retrieve it. Until this port nothing looked
//   at it: not `secret_leak`, not `pii_leak`, and not `ungrounded_ref`, which
//   is the rule this path exists to exercise.
//
// WHERE `ungrounded_ref` ACTUALLY RUNS: here, on the synthesis harness, through
// the runner. `runHarness` builds its own tool record from the messages it sent,
// and a harness turn carries no tool messages - so `backingTools` is empty and
// the rule declines to run, on every harness, by construction. The synthesis
// stage is the one path in Talaria that genuinely HAS a tool record (the search
// hits and their source list ARE the tool results), so it declares `ground`
// (harness/define.ts) and the runner supplies an honest `{ results: true }` for
// it. `server/research.ts` used to run that one rule itself, in a
// `guardSynthesis` function immediately after this harness returned; that
// function is deleted and the rule is in the guard list below, so there is one
// pass, one findings row per fabricated link, and one place to look.
import { z } from 'zod'
import { defineHarness } from '../define'
import { gatewayToolsRefusal, toolPolicyOf, type Transport } from '../transport'
import { buildUpstream, fetchUpstream, recordGatewayUsage, resolveRoute } from '../../llm-gateway'

/** The depth budget the run was started at. Declared here rather than imported
 *  from `server/research.ts` because that module imports this one; `ResearchMode`
 *  there is an alias of this type, so the two can never drift. */
export type ResearchDepth = 'recon' | 'brief' | 'expedition'

// ── 1. The query planner ─────────────────────────────────────────────────────

export interface QueryPlanInput {
  question: string
  /** The mode's per-round budget. It goes in the prompt; the CLAMP is the
   *  adapter's job (`clampQueries`), because a model that returns four when it
   *  was asked for three has still done the job. */
  max: number
  /** Findings so far, one entry per query already run. Empty on round 1, which
   *  is what switches the prompt from "plan angles" to "close the gaps". */
  findingsSoFar: string[]
}

/** One query as the models actually spell it. A bare string is what the prompt
 *  asks for; the three object shapes are what a 7-14B model returns when it
 *  decides a list of strings ought to be a list of records. Accepting them is
 *  the difference between one round trip and two. */
const QUERY_ITEM = z.union([z.string(), z.object({ query: z.string() }), z.object({ q: z.string() }), z.object({ search_query: z.string() })])
type QueryItem = z.infer<typeof QUERY_ITEM>

/** Elements are typed, and that is the whole defense against the bug the old
 *  non-greedy regex had: `[2]` in "According to [2], the gaps are..." is a
 *  perfectly valid JSON array, and the old extractor returned `["2"]` as the
 *  search plan. An array of NUMBERS fails this schema, so `parseJson` walks on
 *  to the next candidate span instead of answering with a citation marker. */
const QUERY_ARRAY = z.array(QUERY_ITEM)

/** The object wrappers are not politeness. `response_format: {type:'json_object'}`
 *  - which the runner sends whenever the model is not known to refuse it -
 *  requires the top-level value to be an OBJECT on most providers, so a harness
 *  that asks for a bare array is fighting its own protocol constraint. The
 *  prompt therefore asks for `{"queries": [...]}` and the bare array stays
 *  accepted for the models that answer with one anyway. */
const QUERY_LIST = z
  .union([QUERY_ARRAY, z.object({ queries: QUERY_ARRAY }), z.object({ search_queries: QUERY_ARRAY })])
  .transform((v): string[] => normalizeQueries(Array.isArray(v) ? v : 'queries' in v ? v.queries : v.search_queries))

const textOf = (item: QueryItem): string =>
  typeof item === 'string' ? item.trim() : 'query' in item ? item.query.trim() : 'q' in item ? item.q.trim() : item.search_query.trim()

const normalizeQueries = (items: QueryItem[]): string[] => items.map(textOf).filter(Boolean)

/** Deduped (case- and whitespace-insensitively) and cut to the round's budget.
 *
 *  Exported because the adapter, the line fallback and the eval fixtures must
 *  all agree on what "N distinct queries" means; three spellings of a dedupe is
 *  how the query count in `research_runs.stats` stops matching the queries that
 *  actually ran. An EMPTY result is a real answer - the persona saying the
 *  question is saturated - and never a failure. */
export function clampQueries(queries: string[], max: number): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const q of queries) {
    const trimmed = q.trim()
    if (!trimmed) continue
    const key = trimmed.toLowerCase().replace(/\s+/g, ' ')
    if (seen.has(key)) continue
    seen.add(key)
    out.push(trimmed)
    if (out.length >= max) break
  }
  return out
}

/** THE LINE FALLBACK, carried over verbatim from `parseQueryList`.
 *
 *  On a small model a numbered list is likelier than a JSON array, and the
 *  filters here are the tolerance that made the old extractor the best of the
 *  six: strip the bullet or the "1." off the front, strip a trailing quote,
 *  drop anything too short to be a query and anything that is a markdown
 *  heading. It is unchanged, including the 8-character floor.
 *
 *  What changed is WHERE it sits. It used to run inside the parser, so a model
 *  that never produced JSON looked exactly like one that did. Now the harness
 *  records the contract failure honestly (`schemaValid: false` on the
 *  `harness_runs` row, with the parser's own sentence in `error`) and the
 *  adapter salvages the run afterwards - and only when the guard found nothing
 *  in the reply, because these strings are sent onward to a search model and
 *  guardrails.ts's cardinal invariant is that flagged content never re-enters
 *  a model's context. */
export function queriesFromLines(raw: string): string[] {
  return raw
    .split('\n')
    .map((l) =>
      l
        .replace(/^[-*\d.\s"']+/, '')
        .replace(/["']$/, '')
        .trim(),
    )
    .filter((l) => l.length > 8 && !l.startsWith('#'))
}

const PLAN_SYSTEM = (max: number) =>
  [
    'You are planning web research in your domain of expertise.',
    `Return ONLY a JSON object of the form {"queries": ["<query>", ...]} with at most ${max} sharp, non-overlapping search queries. No prose.`,
    'Each query is a search-engine query, not a restatement of the research question: name the specific term, source, jurisdiction, product, date range or metric that would settle the point.',
  ].join('\n')

const PLAN_WIDENED =
  'Before you answer, decide what the question actually decomposes into - the distinct claims, actors, timeframes and counter-positions a complete answer would have to cover - and give one query per axis so that no two queries would return the same page. Where the strongest evidence would live in a primary source (a filing, a spec, a changelog, a dataset), aim the query at that source rather than at commentary about it.'

const FINDINGS_CAP = 24_000

const planPrompt = (input: QueryPlanInput): string =>
  input.findingsSoFar.length === 0
    ? `Research question:\n${input.question}`
    : [
        `Research question:\n${input.question}`,
        '',
        `Findings so far:\n${input.findingsSoFar.join('\n\n').slice(0, FINDINGS_CAP)}`,
        '',
        'What is still missing, contradictory, or unverified? Give queries that close those gaps.',
        'If nothing meaningful remains, return {"queries": []} - an empty list ends the research loop, and that is the right answer for a saturated question.',
      ].join('\n')

// ── Eval helpers ─────────────────────────────────────────────────────────────

const WORD = /[a-z0-9]+/g
/** Function words carry no research intent, so counting them as "a term the
 *  question did not contain" would let a plan pass by adding the word "for". */
const STOPWORDS = new Set([
  'a', 'an', 'the', 'and', 'or', 'of', 'in', 'on', 'at', 'to', 'for', 'from', 'by', 'with', 'about', 'into', 'over', 'is', 'are', 'was', 'were', 'be',
  'do', 'does', 'did', 'what', 'which', 'who', 'whom', 'when', 'where', 'why', 'how', 'that', 'this', 'these', 'those', 'it', 'its', 'their', 'they',
  'we', 'our', 'us', 'you', 'your', 'as', 'if', 'not', 'no', 'any', 'all', 'more', 'most', 'than', 'then', 'so', 'up', 'out', 'good', 'enough', 's',
])
const tokens = (s: string): Set<string> => new Set((s.toLowerCase().match(WORD) ?? []).filter((t) => !STOPWORDS.has(t)))
const normalized = (s: string): string =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, ' ')
    .trim()

/** "N distinct, non-empty queries that do not merely restate the question."
 *
 *  Deterministic on purpose - no model grades another model anywhere in this
 *  file. Three failures, and they look nothing alike:
 *
 *    - a query that IS the question, normalized. The model echoed the prompt.
 *    - two queries that are the same query. The model padded to the budget.
 *    - a whole plan that introduces almost no term the question did not already
 *      contain. This is the one that matters, and it is measured over the UNION
 *      of the queries rather than per query, because a plan can be bad in a way
 *      no single query is: three reshuffles of the same nine words each look
 *      "different" from each other and research the same page. One genuinely
 *      new term per query, stopwords excluded, is the bar - which a real plan
 *      clears easily (a term, a jurisdiction, a version, a primary source) and
 *      a reworded one cannot clear at all. */
function planIsUseful(value: string[], question: string, want: { min: number; max: number }): string | null {
  if (value.length < want.min) return `planned ${value.length} quer${value.length === 1 ? 'y' : 'ies'}, wanted at least ${want.min}`
  if (value.length > want.max) return `planned ${value.length} queries, over the ${want.max} the round budgets for`
  for (const q of value) {
    if (!q.trim()) return 'one of the queries was empty'
    if (normalized(q) === normalized(question)) return `query "${q}" is the research question verbatim`
  }
  const keys = value.map((q) => normalized(q))
  if (new Set(keys).size !== keys.length) return 'two of the queries are the same query'
  const asked = tokens(question)
  const novel = new Set<string>()
  for (const q of value) for (const t of tokens(q)) if (!asked.has(t)) novel.add(t)
  return novel.size >= value.length
    ? null
    : `${value.length} queries introduce only ${novel.size} term(s) the question did not already contain - the plan restates rather than researches`
}

export const researchQueriesHarness = defineHarness<QueryPlanInput, string[]>({
  id: 'research-queries',
  label: 'Research planner',
  job: 'Turns a research question - and the gaps left by what has been found so far - into the round’s search queries.',

  // 'json' is the protocol ask; 'instruction-following' is what stops a model
  // answering "here are some angles you could take" instead of a list. Neither
  // is in the FLOOR: a failed plan costs one salvage pass through the line
  // fallback, not a wrong answer, so refusing here would turn a recoverable
  // round into no research at all.
  requires: ['json', 'instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'A model that cannot hold a JSON list still usually answers with a numbered one, and the planner reads that too - it just stops being able to say "nothing is missing", so an expedition runs its full round budget instead of stopping early.',
  },

  // Production ALWAYS pins the requesting agent's own persona as
  // `RunContext.model`: a marketing agent must plan research like a marketer,
  // and that is the feature, not an implementation detail. Empty chain, so a
  // missing pin is a loud no-op rather than a stranger planning the research.
  // See `ModelSpec.chain`.
  model: { chain: [] },

  render: (input, ctx) => [
    { role: 'system', content: ctx.widened ? `${PLAN_SYSTEM(input.max)}\n\n${PLAN_WIDENED}` : PLAN_SYSTEM(input.max) },
    { role: 'user', content: planPrompt(input) },
  ],

  output: { kind: 'json', schema: QUERY_LIST },

  // Null, not a fallback: the adapter has a better answer than any constant -
  // the line-based salvage over the raw reply - and a declared `{ fallback: [] }`
  // here would read to the loop as "the persona says we are saturated" and end
  // the research silently. Those two must never be the same value.
  onFailure: 'null',

  widen: {
    requires: ['instruction-following', 'long-context'],
    note: 'A model that can hold the question and the findings in view at once decomposes the question into axes and aims each query at a primary source, instead of producing three rewordings of the same search.',
  },

  guard: {
    // A query list cannot make a zero-tool claim, cite anything, or report an
    // outage - those three rules would only ever fire on the SUBJECT of the
    // research ("was AWS us-east-1 down in March") and never on a defect.
    // Credentials are the real risk: these strings came from a persona whose
    // soul and memory are in its context, and they are sent onward to a
    // third-party search provider.
    rules: ['secret_leak', 'pii_leak'],
    redact: true,
  },

  evals: [
    {
      name: 'plans distinct angles rather than rewording the question',
      input: {
        question: 'Which EU rules apply to open-weight foundation models released in 2026, and what do they require of the publisher?',
        max: 3,
        findingsSoFar: [],
      },
      check: (value) =>
        planIsUseful(value, 'Which EU rules apply to open-weight foundation models released in 2026, and what do they require of the publisher?', {
          min: 2,
          max: 3,
        }),
    },
    {
      name: 'a narrow factual question still gets more than one angle',
      input: {
        question: 'What did Postgres 17 change about logical replication slot failover?',
        max: 3,
        findingsSoFar: [],
      },
      check: (value) => planIsUseful(value, 'What did Postgres 17 change about logical replication slot failover?', { min: 2, max: 3 }),
    },
    {
      name: 'gap round: queries chase what the findings left open, not what they answered',
      input: {
        question: 'Is our vendor’s SOC 2 posture good enough to process customer payroll data?',
        max: 2,
        findingsSoFar: [
          '### Query: vendor SOC 2 report 2026\nThe vendor published a SOC 2 Type II covering 2025-01 to 2025-12 with no exceptions in the security or availability criteria [1].',
          '### Query: vendor subprocessors payroll\nThe subprocessor list names three processors, one of which is outside the EEA; the transfer mechanism is not stated [2].',
        ],
      },
      check: (value) => {
        const gap = planIsUseful(value, 'Is our vendor’s SOC 2 posture good enough to process customer payroll data?', { min: 1, max: 2 })
        if (gap) return gap
        // The open gap is the unnamed transfer mechanism for the non-EEA
        // subprocessor. A plan that re-searches the SOC 2 report - which the
        // findings already settled - is burning a round on a closed question.
        const known = value.filter((q) => /soc\s*2/i.test(q) && !/subprocessor|transfer|eea|payroll/i.test(q))
        return known.length === value.length ? 'every query re-searches the SOC 2 report, which the findings already settled' : null
      },
    },
    {
      name: 'saturation: an answered question ends the loop instead of burning a round',
      input: {
        question: 'What is the current LTS version of Node.js and when does it reach end of life?',
        max: 3,
        findingsSoFar: [
          '### Query: Node.js LTS release schedule\nNode.js 24 entered active LTS on 2025-10-28 [1]. The official release schedule lists its maintenance window ending 2028-04-30 [1].',
          '### Query: Node.js 24 end of life date\nThe nodejs/Release repository’s schedule.json gives Node.js 24 an end date of 2028-04-30, consistent with the release post [2].',
        ],
      },
      // This is the fixture that separates a model that can say "nothing is
      // missing" from one that cannot. The second is not broken - it just makes
      // every expedition run its full round budget and pay for it.
      check: (value) => (value.length === 0 ? null : `the question is answered twice over and the model still planned ${value.length} more quer${value.length === 1 ? 'y' : 'ies'}`),
    },
  ],
})

// ── 2. The search stage ──────────────────────────────────────────────────────

export interface SearchSource {
  url: string
  title: string | null
  snippet: string | null
}

export interface SearchQueryInput {
  query: string
}

const SEARCH_SYSTEM =
  'You are a research search engine. Answer the query with dense, factual, well-sourced findings. Prefer primary sources and recent data. Note dates and numbers precisely.'

/** THE SEARCH TRANSPORT, and it lives beside its harness rather than in
 *  `harness/run.ts` for one reason: the sources are the product.
 *
 *  Perplexity's sonar family returns its citations OUT OF BAND, in
 *  `search_results` (new) or `citations` (old) on the response body, and the
 *  runner's `gatewayTransport` goes through `completeViaGateway`, which hands
 *  back text and contract drops and throws the rest of the body away. A
 *  research run whose sources are gone is a run with nothing to cite, so this
 *  transport does what `searchStage` used to do by hand - `buildUpstream` +
 *  `fetchUpstream` against the resolved route, metered by `recordGatewayUsage`
 *  exactly as before - and pushes the sources into a sink the adapter holds.
 *
 *  Everything else about the call is the runner's: which model, the capability
 *  floor that keeps a non-search model out, the guard pass, the redaction of
 *  what lands in the synthesis prompt, and the `harness_runs` row.
 *
 *  WHEN THIS SHOULD MOVE: the day `TransportReply` grows a slot for structured
 *  provider metadata, this becomes a `gatewayTransport` option and the sink goes
 *  away. It is a transport shaped like the runner's own, deliberately, so that
 *  move is a lift rather than a rewrite.
 *
 *  IT ALSO KEEPS THE RULE THE RUNNER'S TRANSPORTS KEEP: a request field that
 *  cannot be honored fails the call rather than being dropped. `tools: 'own'`
 *  is the one that matters here — this is the last transport in the tree that
 *  is not in transport.ts, so it is the one place a future `tools: 'own'` on a
 *  gateway-served harness could have gone silently through as a single-shot
 *  completion. `jsonMode` needs no branch: `output.kind` is 'text' on this
 *  harness and `TransportRequest.jsonMode` is derived from that, so there is
 *  nothing to drop; `holdMs` is meaningless off the fleet, for the reason its
 *  own docstring gives. */
export function searchTransport(runId: string, sink: SearchSource[]): Transport {
  return async (req) => {
    if (toolPolicyOf(req) === 'own') throw new Error(gatewayToolsRefusal(req.model))
    const route = await resolveRoute(req.model)
    if (!route) throw new Error(`search model "${req.model}" is not routable`)
    const call = await buildUpstream(route, {
      model: req.model,
      stream: false,
      messages: req.messages.map((m) => ({ role: m.role, content: m.content })),
      ...(req.temperature !== undefined ? { temperature: req.temperature } : {}),
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
    for (const s of sources) sink.push(s)
    return { kind: 'gateway', text: j.choices?.[0]?.message?.content ?? '', toolNames: [], usage: null, contractDropped: false }
  }
}

/** A refusal shaped like an answer. A model with no live search does not error
 *  - it says it cannot browse, or it answers from memory - and both are how an
 *  uncited hallucinated brief starts. Used by the eval below, not at runtime:
 *  the runtime defense is the capability floor. */
const NO_SEARCH_TELL =
  /\b(?:I (?:do not|don'?t|cannot|can'?t) have (?:access to |the ability to )?(?:real[- ]?time|live|current|up[- ]to[- ]date|the internet|browsing)|I (?:cannot|can'?t|am unable to) (?:browse|search the (?:web|internet)|access the internet)|as an AI(?: language model)?,? I)/i

export const researchSearchHarness = defineHarness<SearchQueryInput, string>({
  id: 'research-search',
  label: 'Research search',
  job: 'Runs one planned query against a search-capable model and brings back its findings with the sources attached.',

  // THE DECLARATION THIS PORT EXISTS FOR (audit 1.6). `searchModelFor` resolves
  // an admin's `research-*` role assignment, and `resolveRoleModel` validates
  // exactly one thing about it: that it still ROUTES. So an admin can point
  // Research at any model on the gateway and nothing anywhere notices that it
  // has no web search.
  requires: ['search'],
  floor: {
    capabilities: ['search'],
    // REFUSES, and this is the deliberate part. A search stage on a model with
    // no search does not fail - it answers fluently from training data, the
    // parser finds no `search_results`, and the run assembles a confident brief
    // with a "Sources" section that is either empty or borrowed from a
    // neighbouring query. That is worse than no answer, because it is an answer
    // a human will act on. Below the floor this stage refuses, the adapter
    // records "(search failed: ...)" against the query, and a run where every
    // query refused ends as an ERROR with no citable sources - which is exactly
    // what the pipeline already does when search returns nothing.
    //
    // Unknown is still not missing: a model nobody has probed runs, which is
    // what keeps a fresh self-host working. The refusal fires only on a model
    // that has been positively recorded as unable to search.
    refuseBelow: true,
    note: 'Research needs a model with live web search (Perplexity’s sonar family). A model without it does not fail - it answers from memory, with no sources, in the same confident voice, and the report reads exactly like a researched one. Assign a search model or leave the role on auto.',
  },

  // Production always pins: `searchModelFor(mode)` resolves the per-tier role
  // (`research-recon` / `research-brief` / `research-expedition`) and falls back
  // to the mode's own sonar preference list. That policy is MODE-DEPENDENT and
  // therefore cannot live in a single ModelSpec at all — which is exactly what
  // was wrong with the `role: 'research-brief'` this used to declare: dead in
  // production, and on the one path that could ever have consulted it, it would
  // have handed a recon or an expedition the BRIEF tier's model. Empty chain
  // instead. There is no sane last resort for a search stage anyway —
  // 'first-routable' would hand this harness the very non-search model the
  // floor above exists to keep out.
  model: { chain: [] },

  render: (input) => [
    { role: 'system', content: SEARCH_SYSTEM },
    { role: 'user', content: input.query },
  ],

  output: { kind: 'text' },

  // The caller catches per query and records "(search failed: ...)" in the
  // findings, so one dead query costs one angle rather than the run. A run
  // where every query throws ends with no citable sources, which the pipeline
  // already treats as a hard error.
  onFailure: 'throw',

  guard: {
    // Credentials and PII only. `zero_tool_claim` and `fabricated_outage` read
    // an answer as an AGENT describing its own actions; this answer is a
    // summary of the public web, where "the service was down for four hours" is
    // the finding rather than a confabulation, and both rules would fire on
    // correct output. `ungrounded_ref` is left to the synthesis stage, which is
    // the only place with a tool record to ground against.
    rules: ['secret_leak', 'pii_leak'],
    // Not because the search text is persisted - it is not - but because it is
    // CONCATENATED INTO THE SYNTHESIS PROMPT. guardrails.ts's cardinal
    // invariant is that flagged content never re-enters a model's context, and
    // a search hit quoting a leaked key out of a public paste would otherwise
    // be fed straight to the persona and into the saved report.
    redact: true,
  },

  evals: [
    {
      name: 'answers a time-sensitive question instead of declining to browse',
      input: { query: 'current Node.js LTS version and its end-of-life date' },
      // The discriminating fixture for this role, and it needs no ground truth:
      // a search-capable model answers, and a model without search says so.
      // That is the exact failure `requires: ['search']` is declared against,
      // caught here without asking a second model to grade the first.
      check: (value) => {
        if (NO_SEARCH_TELL.test(value)) return 'declined to answer - the model has no live search'
        if (value.trim().length < 200) return `answered in ${value.trim().length} characters, too thin to be a search result`
        return null
      },
    },
    {
      name: 'answers with dense specifics, not a description of how it would search',
      input: { query: 'EU AI Act obligations for general-purpose AI model providers, effective dates' },
      check: (value) => {
        if (NO_SEARCH_TELL.test(value)) return 'declined to answer - the model has no live search'
        // A stage that returns a plan ("I would look at...") instead of
        // findings gives the synthesis nothing to cite. Dates and numbers are
        // the cheapest deterministic proxy for "this is a finding".
        if (!/\b(?:19|20)\d{2}\b/.test(value)) return 'the answer names no year - a search result on a dated question with no dates in it is a summary of nothing'
        return null
      },
    },
  ],
})

// ── 3. The synthesis stage ───────────────────────────────────────────────────

export interface SynthesisInput {
  question: string
  mode: ResearchDepth
  /** The GLOBAL registry, already renumbered. These indices are the only ones
   *  the model may cite, and the caller strips every marker outside them. */
  sources: Array<{ idx: number; url: string; title: string | null }>
  /** One entry per query run, with citation markers already on global
   *  numbering. Bounded here rather than at the call site so the cap is part of
   *  the declaration. */
  findings: string[]
  /** A search stage threw this run. It is on the INPUT rather than derived
   *  because only the pipeline can see it, and `ground` below needs it to make
   *  an honest claim about error information: `errored: false` on a run where a
   *  query really did die would license `fabricated_outage` to flag a report
   *  that correctly says a source was unreachable. */
  searchFailed: boolean
}

const NOTES_CAP = 80_000

const DEPTH: Record<ResearchDepth, string> = {
  recon: 'a tight, direct answer (a few paragraphs)',
  brief: 'a structured briefing (~1 page): summary up top, then the key findings under headings',
  expedition: 'a thorough report: executive summary, sections per theme, contradictions and open questions called out',
}

const SYNTH_SYSTEM = (mode: ResearchDepth): string =>
  `You are writing a research document in your domain. Requirements:
- Start with a "# " title as your very first characters. No lead-in, no code fences.
- EVERY factual claim carries an inline citation marker like [3] referring to the numbered source list you were given. Never invent a number; never cite a source the findings don't support. Uncited claims are defects.
- Write ${DEPTH[mode]}.
- Do NOT append a sources section — it is added mechanically from the registry.`

/** The widened pass buys RIGOR, not authority: the format, the citation rules
 *  and everything the pipeline does with the document are identical on both
 *  branches. What a long-context model is asked for instead is the thing a
 *  small one reliably cannot do - hold every finding in view at once and
 *  reconcile the ones that disagree, rather than writing down whichever it read
 *  last. A small model asked for this produces a "Contradictions" heading with
 *  nothing under it, which is why it is gated. */
const SYNTH_WIDENED = `
Before you write, read every finding together and note where two sources disagree on a fact, a number or a date. Reconcile them in the document rather than silently choosing one: say what each source claims, cite both, and say which is better supported and why. Where the findings simply do not settle a point the question asked, say so plainly as an open question instead of filling the gap with general knowledge - an unsupported sentence is a defect even when it is true.`

const sourceList = (input: SynthesisInput): string => input.sources.map((s) => `[${s.idx}] ${s.title ?? s.url} — ${s.url}`).join('\n')

const synthPrompt = (input: SynthesisInput): string =>
  `Research question:\n${input.question}\n\nNumbered sources:\n${sourceList(input)}\n\nFindings (citation markers already on global numbering):\n\n${input.findings.join('\n\n').slice(0, NOTES_CAP)}`

const MARKER = /\[(\d{1,2})\]/g

export const researchSynthesisHarness = defineHarness<SynthesisInput, string>({
  id: 'research-synthesis',
  label: 'Research synthesis',
  job: 'Writes the run’s report from the findings and the numbered source registry, in the requesting agent’s own voice.',

  // 'long-context' is the honest ask: the findings run to tens of thousands of
  // characters and a model that truncates them writes a report about the last
  // query only. It is NOT in the floor - a short report grounded in half the
  // findings is still a cited, useful answer, and refusing would throw away a
  // run whose searches have already been paid for.
  requires: ['long-context', 'instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'A model with a short context writes the report from the findings it can still see, so a long expedition reads like a summary of its last few queries. The citations stay honest either way - markers the registry does not know are stripped before the report is saved.',
  },

  // Always pinned to the requesting agent's persona in production: the run's
  // whole premise is that a marketing agent researches like a marketer. Empty
  // chain for the same reason as the planner above (`ModelSpec.chain`).
  model: { chain: [] },

  render: (input, ctx) => [
    { role: 'system', content: ctx.widened ? `${SYNTH_SYSTEM(input.mode)}\n${SYNTH_WIDENED}` : SYNTH_SYSTEM(input.mode) },
    { role: 'user', content: synthPrompt(input) },
  ],

  output: { kind: 'text' },

  // THE GROUNDING HOOK, and this is the harness it was built for.
  //
  // `ungrounded_ref` needs `results` — the text of what the turn's tools
  // actually returned — and `runHarness` derives its tool record from the
  // messages IT sent, which for a harness contain no tool results at all. So
  // the rule self-skips on all 23 harnesses by construction, which is correct
  // everywhere except here. This is the one path in Talaria that genuinely HAS
  // a tool record: the search hits ARE this turn's tool results and the
  // numbered registry is what the model was told it may cite. `ground` hands
  // that over, and `runHarness` supplies `{ results: true, errorInfo: … }`
  // instead of the `{ false, false }` a persona stream is owed — which it must
  // OVERRIDE the fleet branch to do, because these searches happened outside
  // the persona's own tool loop entirely.
  //
  // What it catches, precisely: a URL on an org-policed host, or a UUID, that
  // the document asserts and no search result contains. That is the persona's
  // soul and memory bleeding internal links into a document a human will trust
  // because it looks cited. It does NOT police the `[n]` markers — those are
  // stripped deterministically at the call site against the registry, which is
  // stronger, and the strip count lands in the run's stats.
  //
  // FULL findings, not the `NOTES_CAP` slice the prompt carried: grounding
  // against more than the model saw can only remove false positives. Past
  // `GROUND_RESULTS_CAP` the runner fails open and the rule skips, which is
  // guardrails.ts's own choice about a check whose virtue is being cheap.
  ground: (input) => ({
    tools: ['research_search'],
    results: `${sourceList(input)}\n\n${input.findings.join('\n\n')}`,
    errored: input.searchFailed,
  }),

  // The searches are already paid for and the run's only deliverable is this
  // document, so an empty reply is not "keep what you had" - there is nothing
  // to keep. Throwing lands the message on `research_runs.error`, which is the
  // surface the user is already watching. Before this port an empty reply
  // created an EMPTY ARTIFACT with a generated title and marked the run done.
  onFailure: 'throw',

  widen: {
    requires: ['long-context', 'instruction-following'],
    note: 'A model that holds every finding in view at once reconciles sources that disagree - naming both claims and which is better supported - instead of writing down whichever finding it read last.',
  },

  guard: {
    // `ungrounded_ref` IS in the list now, and it fires — see `ground` above.
    // It used to be deliberately absent, with `server/research.ts` running that
    // one rule itself over a hand-built record, because the runner had no way
    // for a harness to declare grounding material. That workaround is deleted:
    // one pass, one `guard_findings` row per fabricated link, and one place a
    // reader has to look to know which rules police this document.
    //
    // `zero_tool_claim` and `fabricated_outage` stay out for the reason they are
    // out on the search harness: this document describes the world, not the
    // agent's own actions. A report ABOUT an outage would trip both, and
    // `errored` is supplied honestly precisely so that stays a decision rather
    // than an accident.
    rules: ['ungrounded_ref', 'secret_leak', 'pii_leak'],
    // The report is saved as a doc artifact, shared with the run's members and
    // INDEXED INTO THE BRAIN, where every future chat and plan can retrieve it.
    // A credential that reaches it is a credential that reaches all of them.
    redact: true,
  },

  evals: [
    {
      name: 'a titled document that cites only sources it was given',
      input: {
        question: 'What changed in Postgres 17 logical replication, and does it affect failover?',
        mode: 'brief',
        searchFailed: false,
        sources: [
          { idx: 1, url: 'https://www.postgresql.org/docs/17/logical-replication.html', title: 'PostgreSQL 17: Logical Replication' },
          { idx: 2, url: 'https://www.postgresql.org/about/news/postgresql-17-released-2936/', title: 'PostgreSQL 17 Released' },
        ],
        findings: [
          '### Query: postgres 17 logical replication changes\nPostgres 17 lets logical replication slots survive a failover to a standby, so a subscriber no longer has to be re-seeded after promotion [1].',
          '### Query: postgres 17 release notes replication\nThe release announcement lists failover control for logical slots among the headline replication changes [2].',
        ],
      },
      check: (value) => {
        const text = value.trim()
        if (!text.startsWith('# ')) return 'the document does not open with a "# " title'
        const cited = [...text.matchAll(MARKER)].map((m) => Number(m[1]))
        if (cited.length === 0) return 'the document cites nothing - every factual claim was supposed to carry a marker'
        const invented = [...new Set(cited.filter((n) => n !== 1 && n !== 2))]
        if (invented.length) return `cites source${invented.length > 1 ? 's' : ''} ${invented.join(', ')}, which the registry does not have`
        if (/^##+\s*sources\b/im.test(text)) return 'the document appended its own Sources section - the pipeline adds one mechanically, so this duplicates it'
        return null
      },
    },
    {
      name: 'a thin registry does not become a wide set of invented citations',
      input: {
        question: 'When does Node.js 24 reach end of life?',
        mode: 'recon',
        searchFailed: false,
        sources: [{ idx: 1, url: 'https://github.com/nodejs/Release', title: 'nodejs/Release' }],
        findings: ['### Query: node 24 end of life\nThe nodejs/Release schedule gives Node.js 24 an end-of-life date of 2028-04-30 [1].'],
      },
      // The single-source case is where a model that pattern-matches "research
      // report" starts writing [2] and [3] because reports usually have them.
      check: (value) => {
        const cited = [...new Set([...value.matchAll(MARKER)].map((m) => Number(m[1])))]
        const invented = cited.filter((n) => n !== 1)
        if (invented.length) return `only source [1] exists and the document cites ${invented.map((n) => `[${n}]`).join(', ')}`
        if (cited.length === 0) return 'the document cites nothing, though it had a source to cite'
        return null
      },
    },
    {
      name: 'contradictory findings are reported, not quietly resolved',
      input: {
        question: 'How many people does Acme employ?',
        mode: 'brief',
        searchFailed: false,
        sources: [
          { idx: 1, url: 'https://example.com/acme-annual-report-2025', title: 'Acme Annual Report 2025' },
          { idx: 2, url: 'https://example.com/acme-newsroom-headcount', title: 'Acme Newsroom' },
        ],
        findings: [
          '### Query: acme employee count annual report\nThe 2025 annual report states 4,200 employees as of 31 December 2025 [1].',
          '### Query: acme headcount news\nAn Acme newsroom post from March 2026 states "over 5,000 employees worldwide" [2].',
        ],
      },
      check: (value) => {
        // Both numbers present is the deterministic proxy for "the disagreement
        // survived into the document". A report that names only one has picked
        // a winner without telling the reader there was a contest.
        const has4200 = /4[,.\s]?200/.test(value)
        const has5000 = /5[,.\s]?000/.test(value)
        if (has4200 && has5000) return null
        if (!has4200 && !has5000) return 'neither headcount figure reached the document'
        return `reported only ${has4200 ? '4,200' : '5,000'} - the sources disagree and the document does not say so`
      },
    },
  ],
})
