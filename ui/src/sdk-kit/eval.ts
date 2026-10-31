// `talaria-app eval`: replay an app's harness fixtures in the sandbox — against
// replies recorded in the repo, or through the Talaria gateway.
//
// The replay is the host's own (app-harnesses.ts `replayAppCase`): the real
// runner with the model pinned and the run row captured, the fixture's check
// graded the way the fitness sweep grades it. Two things differ from the
// sweep, and both are edges the runner already takes as dependencies:
//
//   · THE TRANSPORT. Recorded: the reply comes from
//     harnesses/recorded/<file>.json, by fixture name and turn, and no model
//     is called. Gateway: the turn goes to the instance's relay
//     (`/api/llm/v1/chat/completions`) on the credential the sandbox holds —
//     never a provider's own endpoint. In a workbench job that is the
//     workbench key, and the job's eval session (written by prepare_env)
//     rides `x-talaria-session`, so the relay books the spend to the ticket
//     the job is for.
//   · THE FACTS ABOUT MODELS. The sandbox has no capability probes and no
//     routing table, so a capability is unknown — and unknown is not missing
//     (capability.ts): the floor never refuses here. Settings the guard
//     reads are their defaults.
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, dirname, join } from 'node:path'
import { listAppHarnesses, replayAppCase, type AppHarness, type CaseReplay } from '@/server/app-harnesses'
import { loadAppHarnesses, type AppHarnessFile } from '@/server/app-load'
import type { HarnessDeps } from '@/server/harness/run'
import {
  gatewayToolsRefusal,
  responseFormatOf,
  toolDefsOf,
  toolNamesOf,
  toolPolicyOf,
  toolWireMessage,
  type ToolCall,
  type Transport,
  type TransportReply,
  type TransportRequest,
} from '@/server/harness/transport'

// ── Recordings ──────────────────────────────────────────────────────────────

/** One reply, as the transport returned it, and a digest of the prompt it
 *  answered: a replay against a prompt that has changed since still runs, and
 *  says so. */
export interface RecordedTurn {
  text: string
  toolCalls?: ToolCall[]
  usage?: { promptTokens: number; completionTokens: number }
  prompt?: string
}

/** harnesses/recorded/<file>.json — every reply one harness's fixtures got,
 *  by fixture name, in turn order (a repair turn is a second reply). */
export interface Recording {
  harness: string
  note?: string
  /** The model the replies came from. */
  model?: string
  cases: Record<string, RecordedTurn[]>
}

export const RECORDED_DIR = 'recorded'

/** Where `file` (a name under harnesses/) keeps its recorded replies. */
export function recordingPath(appDir: string, file: string): string {
  return join(appDir, 'harnesses', RECORDED_DIR, `${basename(file, '.ts')}.json`)
}

export function readRecording(path: string): Recording | string | null {
  if (!existsSync(path)) return null
  try {
    const v = JSON.parse(readFileSync(path, 'utf8')) as Partial<Recording>
    if (!v || typeof v !== 'object' || typeof v.cases !== 'object' || v.cases === null) return `${path} has no "cases" object`
    for (const [name, turns] of Object.entries(v.cases)) {
      if (!Array.isArray(turns) || !turns.every((t) => t && typeof t === 'object' && typeof (t as RecordedTurn).text === 'string')) {
        return `${path}: case ${JSON.stringify(name)} is not a list of { text } replies`
      }
    }
    return { harness: typeof v.harness === 'string' ? v.harness : '', cases: v.cases as Recording['cases'], ...(v.note ? { note: v.note } : {}), ...(v.model ? { model: v.model } : {}) }
  } catch (e) {
    return `${path} could not be read: ${e instanceof Error ? e.message : String(e)}`
  }
}

export function writeRecording(path: string, rec: Recording): void {
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, `${JSON.stringify(rec, null, 2)}\n`)
}

/** The prompt a request carries, as one digest. */
export function promptDigest(req: Pick<TransportRequest, 'messages'>): string {
  const h = createHash('sha256')
  for (const m of req.messages) h.update(`${m.role}\0${m.content}\0`)
  return h.digest('hex').slice(0, 16)
}

/** Serve `turns` in order. A request past the last recorded reply fails the
 *  turn with the sentence that says what to do; `drift` is called when a
 *  turn's prompt is not the one the reply was recorded against. */
export function recordedTransport(turns: readonly RecordedTurn[], drift: (turn: number) => void = () => {}): Transport {
  let next = 0
  return async (req) => {
    const turn = turns[next]
    if (!turn) {
      throw new Error(
        `the recording holds ${turns.length} ${turns.length === 1 ? 'reply' : 'replies'} for this fixture and the runner asked for another — re-record it (talaria-app eval --gateway --model <id> --record)`,
      )
    }
    if (turn.prompt && turn.prompt !== promptDigest(req)) drift(next)
    next++
    const toolCalls = turn.toolCalls
    return {
      kind: 'gateway',
      text: turn.text,
      toolNames: toolCalls ? toolNamesOf(toolCalls) : [],
      ...(toolCalls ? { toolCalls } : {}),
      usage: turn.usage ?? null,
      contractDropped: false,
    }
  }
}

/** Wrap `base`, keeping every reply it returns as a `RecordedTurn`. */
export function recordingTransport(base: Transport, into: RecordedTurn[]): Transport {
  return async (req) => {
    const reply = await base(req)
    into.push({
      text: reply.text,
      ...(reply.toolCalls ? { toolCalls: reply.toolCalls } : {}),
      ...(reply.usage ? { usage: reply.usage } : {}),
      prompt: promptDigest(req),
    })
    return reply
  }
}

// ── The gateway ─────────────────────────────────────────────────────────────

export interface GatewayTarget {
  /** The instance's origin, as this sandbox reaches it. */
  url: string
  /** A gateway credential: the workbench key in a job, an operator's own key
   *  anywhere else. */
  key: string
  /** The job's eval session — the relay books the turn under the flow the
   *  job recorded for it (its ticket). */
  session?: string
}

const COMPLETIONS = '/api/llm/v1/chat/completions'

/** The request body the relay gets for one turn: the body the host's own
 *  gateway transport builds (`gatewayToolTurn`), to an HTTP door instead of
 *  an in-process one. */
export function gatewayBody(req: TransportRequest): Record<string, unknown> {
  const defs = toolDefsOf(req)
  const body: Record<string, unknown> = {
    model: req.model,
    messages: req.messages.map(toolWireMessage),
    stream: false,
    ...(defs.length
      ? {
          tools: defs.map((d) => ({ type: 'function', function: { name: d.name, description: d.description, parameters: d.parameters } })),
          tool_choice: 'auto',
          reasoning_effort: 'none',
        }
      : {}),
  }
  if (req.temperature !== undefined) body.temperature = req.temperature
  if (!defs.length && req.effort) body.reasoning_effort = req.effort
  const format = responseFormatOf(req)
  if (format) body.response_format = format
  return body
}

interface WireToolCall {
  id?: string
  function?: { name?: string; arguments?: string }
}

/** Turns through the relay over HTTP. A non-2xx answer fails the turn with
 *  the relay's own sentence (it never carries a provider's prose). */
export function gatewayTransport(target: GatewayTarget, fetchImpl: typeof fetch = fetch): Transport {
  return async (req): Promise<TransportReply> => {
    if (toolPolicyOf(req) === 'own') throw new Error(gatewayToolsRefusal(req.model))
    const res = await fetchImpl(`${target.url.replace(/\/+$/, '')}${COMPLETIONS}`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${target.key}`,
        'content-type': 'application/json',
        ...(target.session ? { 'x-talaria-session': target.session } : {}),
      },
      body: JSON.stringify(gatewayBody(req)),
    })
    const text = await res.text()
    let j: {
      choices?: Array<{ message?: { content?: string | null; tool_calls?: WireToolCall[] } }>
      usage?: { prompt_tokens?: number; completion_tokens?: number }
      error?: { message?: string } | string
    } = {}
    try {
      j = JSON.parse(text)
    } catch {
      // A body that is not JSON is reported by status below.
    }
    if (!res.ok) {
      const why = typeof j.error === 'string' ? j.error : (j.error?.message ?? text.slice(0, 200))
      throw new Error(`the gateway answered ${res.status}: ${why}`)
    }
    const message = j.choices?.[0]?.message
    const toolCalls: ToolCall[] = (message?.tool_calls ?? []).flatMap((tc, i) =>
      tc.function?.name ? [{ name: tc.function.name, args: tc.function.arguments ?? '', id: tc.id ?? `call_${i}` }] : [],
    )
    const offered = toolDefsOf(req).length > 0
    return {
      kind: 'gateway',
      text: message?.content ?? '',
      toolNames: toolNamesOf(toolCalls),
      ...(offered || toolCalls.length ? { toolCalls } : {}),
      usage: j.usage ? { promptTokens: j.usage.prompt_tokens ?? 0, completionTokens: j.usage.completion_tokens ?? 0 } : null,
      contractDropped: false,
    }
  }
}

/** The models the relay serves this credential, for the sentence that asks
 *  for `--model`. Empty when the list could not be read. */
export async function gatewayModels(target: GatewayTarget, fetchImpl: typeof fetch = fetch): Promise<string[]> {
  try {
    const res = await fetchImpl(`${target.url.replace(/\/+$/, '')}/api/llm/v1/models`, { headers: { authorization: `Bearer ${target.key}` } })
    if (!res.ok) return []
    const j = (await res.json()) as { data?: Array<{ id?: unknown }> }
    return (j.data ?? []).flatMap((m) => (typeof m.id === 'string' ? [m.id] : []))
  } catch {
    return []
  }
}

/** The eval session prepare_env writes for a workbench job: the nearest
 *  `.talaria/eval-session.json` at or above the app. */
export interface EvalSession {
  key: string
  jobId?: string
}

export function findEvalSession(appDir: string): EvalSession | null {
  let dir = appDir
  for (let i = 0; i < 6; i++) {
    const p = join(dir, '.talaria', 'eval-session.json')
    if (existsSync(p)) {
      try {
        const v = JSON.parse(readFileSync(p, 'utf8')) as Partial<EvalSession>
        if (typeof v.key === 'string' && /^[\x21-\x7e]{1,128}$/.test(v.key)) return { key: v.key, ...(typeof v.jobId === 'string' ? { jobId: v.jobId } : {}) }
      } catch {
        // An unreadable file is no session; the turn is still booked.
      }
      return null
    }
    const up = dirname(dir)
    if (up === dir) break
    dir = up
  }
  return null
}

// ── The run ─────────────────────────────────────────────────────────────────

/** The runner's edges that read the instance, answered the way a fresh
 *  sandbox can answer them: no routing table, no probes, no effort
 *  preferences. The model is always pinned, so nothing resolves one. */
export const SANDBOX_DEPS: Partial<HarnessDeps> = {
  resolveModel: async () => null,
  slotEffort: async () => null,
  routing: async (model) => ({ endpoints: [], upstreamModel: model }),
  personaKeys: async () => [],
  missingCapabilities: async () => [],
  capabilities: async () => ({}),
  reach: async () => ({}),
}

export type CaseStatus = 'pass' | 'fail' | 'gap' | 'refused' | 'error' | 'unrecorded'

export interface CaseResult {
  harness: string
  file: string
  case: string
  band: string
  status: CaseStatus
  /** The fixture's sentence, or the run's error. */
  reason: string | null
  source: 'recorded' | 'gateway'
  model: string
  repairs: number
  replies: number
  promptTokens: number
  completionTokens: number
  /** Recorded replies answered a prompt other than the one the harness
   *  renders now. */
  drifted: boolean
}

export interface EvalReport {
  cases: CaseResult[]
  /** Harness files the listing skipped, with why — they have no fixtures to run. */
  skipped: string[]
  recorded: string[]
  ok: boolean
}

export interface EvalOptions {
  slug: string
  appName: string
  appDir: string
  harness?: string
  case?: string
  /** Absent: recorded replies. */
  gateway?: GatewayTarget & { model: string }
  /** With `gateway`: write what the gateway answered back to the recordings. */
  record?: boolean
  /** Test seam. */
  fetchImpl?: typeof fetch
  /** Test seam: the harness files, instead of the built harnesses.js. */
  files?: (slug: string) => Promise<AppHarnessFile[]>
  /** Test seam: runner edges over `SANDBOX_DEPS` (never the transport). */
  deps?: Partial<HarnessDeps>
}

const statusOf = (r: CaseReplay): { status: CaseStatus; reason: string | null } => {
  if (r.refused) return { status: 'refused', reason: r.error }
  if (r.verdict === 'pass') return { status: 'pass', reason: null }
  if (r.verdict === 'fail') return { status: 'fail', reason: r.reason }
  if (r.verdict === 'gap') return { status: 'gap', reason: r.reason }
  return { status: 'error', reason: r.error ?? (r.schemaValid ? 'the run produced no value' : 'the reply did not hold the output contract') }
}

/** Replay the fixtures of the app's built harnesses. */
export async function runEvals(opts: EvalOptions): Promise<EvalReport> {
  const problems: string[] = []
  const listing = await listAppHarnesses({
    enabled: async () => [{ slug: opts.slug, name: opts.appName }],
    files: opts.files ?? ((slug: string) => loadAppHarnesses(slug)),
    warn: () => {},
  })
  for (const p of listing.problems) problems.push(`harnesses/${p.file} — ${p.reason}`)
  const harnesses = listing.harnesses.filter((h) => h.slug === opts.slug && (!opts.harness || h.def.id === opts.harness || h.file === opts.harness || basename(h.file, '.ts') === opts.harness))
  const cases: CaseResult[] = []
  const recorded: string[] = []
  for (const h of harnesses) {
    cases.push(...(await evalHarness(h, opts, recorded)))
  }
  const ok = cases.length > 0 && cases.every((c) => c.status === 'pass' || c.status === 'gap' || c.status === 'refused')
  return { cases, skipped: problems, recorded, ok }
}

async function evalHarness(h: AppHarness, opts: EvalOptions, recordedPaths: string[]): Promise<CaseResult[]> {
  const path = recordingPath(opts.appDir, h.file)
  const existing = readRecording(path)
  if (typeof existing === 'string') throw new Error(existing)
  const fixtures = (h.def.evals ?? []).filter((e) => !opts.case || e.name === opts.case)
  const out: CaseResult[] = []
  const fresh: Recording['cases'] = {}
  for (const fixture of fixtures) {
    const base: Omit<CaseResult, 'status' | 'reason' | 'model' | 'repairs' | 'replies' | 'promptTokens' | 'completionTokens' | 'drifted'> = {
      harness: h.def.id,
      file: h.file,
      case: fixture.name,
      band: fixture.band ?? 'standard',
      source: opts.gateway ? 'gateway' : 'recorded',
    }
    let transport: Transport
    let model: string
    let drifted = false
    const turns: RecordedTurn[] = []
    if (opts.gateway) {
      model = opts.gateway.model
      const wire = gatewayTransport(opts.gateway, opts.fetchImpl)
      transport = opts.record ? recordingTransport(wire, turns) : wire
    } else {
      const replies = existing?.cases[fixture.name]
      if (!replies) {
        out.push({
          ...base,
          status: 'unrecorded',
          reason: `no reply recorded in ${path.slice(opts.appDir.length + 1)} — record one: talaria-app eval --gateway --model <id> --record`,
          model: '',
          repairs: 0,
          replies: 0,
          promptTokens: 0,
          completionTokens: 0,
          drifted: false,
        })
        continue
      }
      model = existing?.model ? `recorded:${existing.model}` : 'recorded'
      transport = recordedTransport(replies, () => {
        drifted = true
      })
      turns.push(...replies)
    }
    let calls = 0
    const counted: Transport = async (req) => {
      calls++
      return transport(req)
    }
    const replay = await replayAppCase(h, fixture.name, model, { deps: { ...SANDBOX_DEPS, ...opts.deps, transport: counted } })
    if (!replay) continue
    const { status, reason } = statusOf(replay)
    out.push({
      ...base,
      status,
      reason,
      model,
      repairs: replay.repairs,
      replies: calls,
      promptTokens: replay.promptTokens,
      completionTokens: replay.completionTokens,
      drifted,
    })
    if (opts.gateway && opts.record && turns.length > 0) fresh[fixture.name] = turns
  }
  if (opts.gateway && opts.record && Object.keys(fresh).length > 0) {
    writeRecording(path, {
      harness: h.def.id,
      note: existing && typeof existing !== 'string' && existing.note ? existing.note : 'Replies talaria-app eval replays offline, by fixture name.',
      model: opts.gateway.model,
      cases: { ...(existing && typeof existing !== 'string' ? existing.cases : {}), ...fresh },
    })
    recordedPaths.push(path)
  }
  return out
}

const LABEL: Record<CaseStatus, string> = {
  pass: 'PASS',
  fail: 'FAIL',
  gap: 'GAP',
  refused: 'REFUSED',
  error: 'ERROR',
  unrecorded: 'MISSING',
}

/** The report as a person reads it. */
export function formatEvalReport(r: EvalReport, appDir: string): string {
  const out: string[] = []
  let current = ''
  for (const c of r.cases) {
    const head = `${c.harness} (harnesses/${c.file}) · ${c.source === 'gateway' ? `gateway, ${c.model}` : c.model || 'recorded'}`
    if (head !== current) {
      if (out.length) out.push('')
      out.push(head)
      current = head
    }
    const detail = c.status === 'unrecorded' ? '' : ` (${c.replies} ${c.replies === 1 ? 'reply' : 'replies'}${c.repairs ? `, ${c.repairs} repair${c.repairs === 1 ? '' : 's'}` : ''}${c.source === 'gateway' ? `, ${c.promptTokens}+${c.completionTokens} tokens` : ''})`
    out.push(`  ${LABEL[c.status].padEnd(8)} ${c.case}${detail}`)
    if (c.reason) out.push(`           ${c.reason}`)
    if (c.drifted) out.push('           the recorded reply answered an earlier prompt — re-record when the change is deliberate')
  }
  for (const s of r.skipped) out.push(`  SKIPPED  ${s}`)
  if (r.cases.length === 0 && r.skipped.length === 0) out.push('no harness fixtures to run (harnesses/*.ts with evals)')
  for (const p of r.recorded) out.push(`recorded ${p.slice(appDir.length + 1)}`)
  const count = (s: CaseStatus) => r.cases.filter((c) => c.status === s).length
  const tally = (['pass', 'fail', 'gap', 'refused', 'error', 'unrecorded'] as const)
    .filter((s) => count(s) > 0)
    .map((s) => `${count(s)} ${LABEL[s].toLowerCase()}`)
    .join(', ')
  if (r.cases.length) out.push('', `${r.cases.length} ${r.cases.length === 1 ? 'fixture' : 'fixtures'}: ${tally}`)
  return out.join('\n')
}
