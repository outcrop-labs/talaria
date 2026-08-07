// THE ONE DOOR for agent-authored text on its way to a human.
//
// THE HOLE THIS CLOSES, and it is a DIFFERENT CLASS from everything the harness
// port fixed. `runHarness` guards model OUTPUT — every harness reply is scanned
// before it is used, redacted before it is persisted, and recorded against the
// model that produced it. But an agent also writes to humans through its MCP
// TOOLS, and a tool ARGUMENT is model output that never touched a harness:
//
//   mcp `comment`          → a ticket comment          (tasks.addComment)
//   mcp `post_to_channel`  → a post everybody reads     (channels.insertChannelMessage)
//   mcp `message_user`     → a DM + an inbox notification (outreach.agentMessageUser)
//
// The net effect before this file existed, stated as the bug: an outreach
// check-in's REPORT line was guarded and redacted by the runner, and the DM the
// agent actually sent during that same turn was not. An agent that echoed its
// own `tak_…` credential into a teammate's inbox was not flagged, and the stored
// copy kept the credential forever.
//
// ONE DOOR, NOT THREE. Three write paths, one question, so the answer is written
// once and each path calls it — the same method as `logTicket` in
// workbench-mcp.ts, and for the same reason: three checks at three sites is
// three chances to write the check differently and a fourth site next quarter
// that has none. The door lives INSIDE the persistence functions rather than
// beside them, so a caller cannot EXPRESS the ungated write: there is no flag to
// pass and no sibling function to reach for.
//
// WHO COUNTS AS AN AGENT. `WriteAuthor` is `WorkbenchActor`'s shape and carries
// its reasoning: a caller that KNOWS it is writing as an agent says so
// (`{ agent: model }` — channels has an author_type column, so it knows); a
// caller holding a bare author string gets it VERIFIED against agent_defs, so
// the human door cannot be used to launder an agent write back in. Like
// `agentBehind`, the lookup deliberately does not filter on `enabled` — a
// disabled agent's text must not slip the guard by being read as a person's.
//
// WHICH RULES RUN, and why only those. `guardText` runs the GATE-SAFE rules —
// `secret_leak` and `pii_leak` — which is exactly right here and not a
// simplification: this door holds a finished string and no tool record, so
// `zero_tool_claim` and `fabricated_outage` have nothing to be true against. An
// MCP comment that says "I opened the PR" is a claim whose backing tool ran in a
// different process; running that rule here would flag honest work. Rules that
// need what we cannot supply are skipped rather than guessed — `Available` is
// the same idea one layer down.
//
// A SECRET REDACTS AND RECORDS. IT NEVER BLOCKS. Deliberate, and the reasoning
// is the point:
//   · These three tools are how an agent does its job. A blocked comment is not
//     a safe outcome — the human never learns the thing, and the agent is told
//     the write landed or is told a reason it will then try to argue with.
//   · The detectors are regex families. Their precision is high, not perfect, and
//     the cost of a false positive here is a silently swallowed message from a
//     teammate, which is its own kind of harm and an invisible one.
//   · Redaction is lossless in the way that matters: the sentence survives, the
//     credential does not. That IS the security outcome — the persisted copy,
//     and every transcript, search index and notification built from it, is
//     clean.
//   · The org already has the graduated control. `observe` records, `strict`
//     scrubs. An operator who wants a hard posture has one, and it removes the
//     credential rather than the message.
//
// WHAT THE TEXT IS GROUNDED AGAINST. Both gate-safe rules are groundable
// (guardrails.ts), so a caller that can name the material the agent was working
// FROM — the ticket being triaged, the thread being replied to — passes it as
// `input` and a span that came out of that material stops being treated as the
// model's invention. It matters most here: a triage outcome quoting the
// reporter's order number is the exact shape the validation round measured, and
// this door both persists that text and indexes it. Callers with nothing honest
// to name pass nothing and behave exactly as before, which is what every one of
// them does today.
//
// WHAT NEVER GOES BACK TO THE AGENT. guardrails.ts's cardinal invariant: flagged
// CONTENT never re-enters a model's context. This door returns text and nothing
// else to its callers' return values — no finding, no message, and above all no
// `snippet`, which is a verbatim excerpt of the flagged span and would put the
// credential straight back into the agent's context while ostensibly enforcing
// the rule against doing so. The agent sees its own sentence with the secret
// gone, which is a redaction and not a quotation.
import { db } from './db/pg'
import {
  getGuardConfig,
  guardText,
  needsRedaction,
  recordFindings,
  redactSecrets,
  type Finding,
  type GuardConfig,
  type GuardMode,
} from './guardrails'

/** The write path a finding came from. It becomes the `caller` on the
 *  guard_findings row, so `guard_findings.model` keeps meaning "this model's
 *  confabulation rate" (what the fitness page reads) while `caller` says which
 *  door the text came through. */
export type AgentWriteSurface = 'ticket-comment' | 'channel-post' | 'direct-message' | 'ticket-write' | 'capability-gap'

/** `WorkbenchActor`'s shape, for `logTicket`'s reason: the object form is a
 *  caller that knows it is writing as an agent, the bare string is an author
 *  field that might be either and is VERIFIED rather than believed. */
export type WriteAuthor = string | { agent: string }

export interface GuardedAgentWrite {
  /** What to persist, deliver and notify with. Identical to the input unless
   *  strict mode redacted a credential out of it. */
  text: string
  /** For the CALLER's own bookkeeping (pinning a caveat onto a row the UI
   *  renders). Never returned to the agent — see the file header. */
  findings: Finding[]
  /** The agent this text was attributed to, or null when the author is a human
   *  (a person's typing is not model output and is not guarded here). */
  agent: string | null
  mode: GuardMode
  redacted: boolean
}

export interface AgentWriteDeps {
  /** Does this author string name a fleet agent? */
  isAgent: (name: string) => Promise<boolean>
  guardText: (text: string, input?: string) => Promise<Finding[]>
  guardConfig: () => Promise<GuardConfig>
  recordFindings: typeof recordFindings
}

/** What the agent was writing FROM, when the caller knows. See the file header:
 *  a span already present in this material is the user's own data and neither a
 *  finding against the agent nor (for PII) a rewrite of what gets stored. */
export interface AgentWriteOptions {
  input?: string
}

const REAL_DEPS: AgentWriteDeps = {
  isAgent: async (name) => {
    const sql = await db()
    // No `enabled` filter, on purpose — see the header. One indexed lookup on a
    // small table, the same cost `logTicket` pays for the same guarantee.
    const rows = await sql`select 1 from agent_defs where model = ${name} limit 1`
    return rows.length > 0
  },
  guardText,
  guardConfig: getGuardConfig,
  recordFindings,
}

/** Run the gate-safe guard rules over text an agent wrote, record what they
 *  find against that agent, and hand back what is safe to store.
 *
 *  `guardText` is reused rather than rewired: it is already the "gate-safe rules
 *  over plain text" entry point (judge.ts's pre-check is the other caller), it
 *  reads the org's mode itself, and it returns [] when the guard is off. A
 *  second wiring of the same rules is how two callers end up disagreeing about
 *  which rules "the guard" means.
 *
 *  Never throws. A guard that fails closed on a database hiccup would take down
 *  commenting, posting and DMs — the failure mode would be worse than the leak
 *  it prevents, and it would be indistinguishable from Talaria being broken. */
export async function guardAgentWrite(
  surface: AgentWriteSurface,
  by: WriteAuthor,
  text: string,
  deps: Partial<AgentWriteDeps> = {},
  opts: AgentWriteOptions = {},
): Promise<GuardedAgentWrite> {
  const d: AgentWriteDeps = { ...REAL_DEPS, ...deps }
  const clean = (agent: string | null, mode: GuardMode = 'off'): GuardedAgentWrite => ({
    text,
    findings: [],
    agent,
    mode,
    redacted: false,
  })
  if (!text.trim()) return clean(null)

  const agent = typeof by === 'string' ? ((await d.isAgent(by).catch(() => false)) ? by : null) : by.agent
  if (!agent) return clean(null)

  const findings = await d.guardText(text, opts.input).catch(() => [] as Finding[])
  if (!findings.length) return clean(agent)

  // A non-empty result proves the guard is not off, so the mode read below is
  // only ever asking WHICH on-mode. If that read fails we keep the org's default
  // posture (observe): record the finding, change nothing. Guessing `strict`
  // would silently rewrite a teammate's message under a policy nobody set.
  const mode = (await d.guardConfig().catch(() => null))?.mode ?? 'observe'

  // `model` is the AGENT, so the fitness page's per-model confabulation rate
  // counts what an agent leaked through its tools alongside what it leaked
  // through a harness reply. `endpoint: 'fleet'` is guardChatReply's spelling
  // for the same fact: the text was produced inside the agent, not by a gateway
  // completion we placed.
  await d.recordFindings(findings, { caller: `${surface}:${agent}`, model: agent, endpoint: 'fleet', mode }).catch(() => {})

  if (mode !== 'strict' || !needsRedaction(findings)) return { text, findings, agent, mode, redacted: false }
  // `input` again, because the two halves have to agree: a span the guard
  // declined to file a finding about is a span the redactor must decline to
  // rewrite (PII), and a credential it declined to blame the agent for is still
  // one it removes from the stored copy (secrets). guardrails.ts owns which is
  // which; this call is only obliged to hand it the same material both times.
  const safe = redactSecrets(text, opts.input)
  return { text: safe.text, findings, agent, mode, redacted: safe.redacted }
}

/** The same door for a write made of SEVERAL agent-authored fields at once.
 *
 *  A ticket is the case: `create_ticket` and `triage_ticket` carry a title, a
 *  description, and (on triage) an outcome and a resolution, and every one of
 *  them is a tool argument — model output that never touched a harness, exactly
 *  the class the header describes. They were the fourth and fifth doors with no
 *  guard on them: `indexTicket` put the raw text into the retrieval collection
 *  another agent searches, `notifyMentions` mailed it, and no `guard_findings`
 *  row ever said the agent leaked anything, so the fitness page undercounted the
 *  most-used agent write surface in the product.
 *
 *  ONE PASS, NOT ONE PER FIELD: the fields are scanned together so a ticket
 *  produces one finding row rather than four, and then redacted individually so
 *  each column gets its own clean value. Absent fields stay absent — an
 *  `undefined` in a patch means "don't touch this column" and must survive. */
export async function guardAgentFields<T extends Record<string, string | null | undefined>>(
  surface: AgentWriteSurface,
  by: WriteAuthor,
  fields: T,
  deps: Partial<AgentWriteDeps> = {},
  opts: AgentWriteOptions = {},
): Promise<T> {
  const entries = Object.entries(fields) as Array<[keyof T & string, string | null | undefined]>
  const joined = entries
    .map(([, v]) => v)
    .filter((v): v is string => typeof v === 'string' && v.trim().length > 0)
    .join('\n\n')
  if (!joined) return fields
  const guarded = await guardAgentWrite(surface, by, joined, deps, opts)
  if (!guarded.redacted) return fields
  const out = { ...fields }
  // The SAME grounding material per field as the joined pass used, or the two
  // passes disagree: the join decides whether anything is redacted at all and
  // these calls decide what each column ends up holding.
  for (const [k, v] of entries) if (typeof v === 'string' && v) out[k] = redactSecrets(v, opts.input).text as T[typeof k]
  return out
}
