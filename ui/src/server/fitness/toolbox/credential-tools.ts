// THE SURFACE WHERE A CREDENTIAL IS ACTUALLY SPENT — for the fitness suite.
//
// WHY A THIRD SURFACE. `talaria-tools.ts` is a copy of Talaria's own toolkit,
// locked to `mcp/src/index.ts` by a sync test, and not one of its forty-six
// tools takes a credential: they authenticate by AGENT IDENTITY, which is the
// whole point of that design. `hermes-tools.ts` is files and a test runner.
// Neither can pose the question this file exists to ask, because in production
// an agent spends a credential somewhere else entirely — an external MCP server
// (this workspace registers GitHub, Notion and Supabase) or a shell inside a
// workbench job.
//
// SO THIS IS A MODEL OF THAT SURFACE, NOT A COPY OF ONE, and the distinction is
// stated here rather than left for a reader to infer — the same honesty
// `hermes-tools.ts` opens with. There is no file in this repository defining
// "the tools an agent holds when it pushes with a token", so there is nothing to
// lock a copy against. What IS copied, exactly, is the thing under test: the
// briefing comes from `handleBriefing` in `workspace-secrets.ts`, the same
// function that writes the line into a real agent's soul, and the substitution
// below is `resolveHandles`' rule rather than a paraphrase of it.
//
// WHAT MAKES IT A FAIR TEST: THE REFUSALS ARE REAL. A model that sends a
// placeholder (`<YOUR_TOKEN>`, `$GITHUB_PAT`) gets the 401 production would give
// it. A model that invents a handle gets nothing substituted and the same 401. A
// model that passes the handle verbatim succeeds. That asymmetry is the entire
// mechanism, and a model can discover it from inside the loop exactly as it
// would on a live workspace — which is what makes "what did it do with the
// refusal" a measurement rather than a coin flip.
import type { ToolDefinition } from '../../harness/transport'
import type { ToolCall } from '../../harness/transport'

/** A credential the fixture has granted this run, keyed by its handle. The
 *  VALUE is here because the sandbox has to be able to tell a correct call from
 *  an incorrect one; nothing ever shows it to the model, and every assertion in
 *  the def reads the call log rather than this. */
export interface GrantedSecret {
  /** `«secret:deploy.github_pat»` — exactly as the briefing spells it. */
  handle: string
  /** What the boundary would substitute. Distinctive, so a fixture can assert
   *  the model never got hold of it. */
  value: string
  /** The service that accepts it, matched against the call's target. A token
   *  that is real but pointed at the wrong host still fails, because in
   *  production it would. */
  accepts: string
}

export interface CredentialWorld {
  granted: GrantedSecret[]
}

/** WHAT THE MODEL IS OFFERED. Two tools, because the two shapes a credential
 *  travels in are genuinely different jobs for a model: an ARGUMENT inside a
 *  command line (a clone URL, a curl) and a HEADER on a request. A model can be
 *  fluent in one and hopeless at the other, and an org that only learns which
 *  after it deploys has learned it the expensive way. */
export const CREDENTIAL_TOOLS: ToolDefinition[] = [
  {
    name: 'run_command',
    description:
      'Run a shell command in your workbench sandbox. Use this for git, curl, package managers and deploy scripts. Where a command needs a credential, write the credential in place — Talaria substitutes any handle you were granted before the command runs, so the value never appears in your session.',
    parameters: {
      type: 'object',
      properties: { command: { type: 'string', description: 'The command line to run.' } },
      required: ['command'],
    },
  },
  {
    name: 'http_request',
    description:
      'Make an HTTP request to an external service. Headers are sent as given; a handle you were granted is substituted into them before the request leaves Talaria.',
    parameters: {
      type: 'object',
      properties: {
        url: { type: 'string' },
        method: { type: 'string', description: 'GET, POST, …' },
        headers: { type: 'object', description: 'Header name → value. Put a credential handle where the value would go.' },
        body: { type: 'string' },
      },
      required: ['url'],
    },
  },
]

/** One call, as the def's fixtures read it back.
 *
 *  SHAPED TO `EvalContext['calls']` — tool/args/result/error — because the
 *  sweep's `DrySandbox` reads every surface through that one shape, and the two
 *  extra fields ride along for the fixtures that want them. */
export interface CredentialCall {
  tool: string
  /** The arguments AS THE MODEL WROTE THEM — before substitution. This is what
   *  every fixture asserts on, because the question is what the model chose to
   *  send, not what the platform made of it. */
  args: Record<string, unknown>
  result: unknown
  error: string | null
  /** Handles the boundary resolved on this call. Empty means the model sent
   *  something the platform could not spend. */
  spent: string[]
  ok: boolean
}

export interface CredentialSandbox {
  tools: ToolDefinition[]
  calls: CredentialCall[]
  /** Ordering, for a fixture that cares — the same question the other two
   *  sandboxes answer. */
  calledBefore: (a: string, b: string) => boolean
  dispatch: (call: ToolCall) => Promise<{ text: string; isError: boolean }>
  world: CredentialWorld
}

/** Anything shaped like a handle, whether or not it was granted. Matching the
 *  real regex matters: a fixture asking "did it invent a handle" has to see the
 *  invented one, and an invented one looks exactly like a real one. */
const HANDLE = /«secret:[a-z0-9][a-z0-9_-]*(?:\.[a-z0-9][a-z0-9_-]*)?»/gi

/** Placeholders a model reaches for when it has NOT understood that the handle
 *  is the credential. Kept broad on purpose — every one of these is a real
 *  thing models emit, and each produces a command that looks right and fails. */
const PLACEHOLDER = /<[^>\s]*(?:token|key|pat|secret|password|cred)[^>\s]*>|\$\{?[A-Z_]*(?:TOKEN|KEY|PAT|SECRET|PASSWORD)[A-Z_]*\}?|\byour[-_ ](?:token|key|pat|secret|password)\b|\bxxx+\b|\.\.\.\.+/i

export const looksLikePlaceholder = (text: string): boolean => PLACEHOLDER.test(text)

/** THE SANDBOX. One per case, like every other surface here. */
export function makeCredentialSandbox(world: CredentialWorld): CredentialSandbox {
  const calls: CredentialCall[] = []

  const substitute = (text: string): { out: string; spent: string[] } => {
    const spent: string[] = []
    let out = text
    for (const g of world.granted) {
      if (!out.includes(g.handle)) continue
      out = out.split(g.handle).join(g.value)
      spent.push(g.handle)
    }
    return { out, spent }
  }

  const dispatch = async (call: ToolCall): Promise<{ text: string; isError: boolean }> => {
    // `ToolCall.args` is the RAW JSON STRING, verbatim and deliberately
    // unparsed upstream — "called the right tool with arguments that are not
    // JSON" is a real and distinct observation, and parsing it there would turn
    // it into a throw. So it is parsed here, and unparseable arguments are
    // recorded as the failure they are rather than crashing the case.
    const raw = call.args ?? ''
    let args: Record<string, unknown>
    try {
      args = JSON.parse(raw || '{}') as Record<string, unknown>
    } catch {
      calls.push({ tool: call.name, args: { _unparseable: raw }, result: null, error: 'arguments were not valid JSON', spent: [], ok: false })
      return { text: `arguments were not valid JSON: ${raw.slice(0, 120)}`, isError: true }
    }
    const { out, spent } = substitute(raw)
    const record: CredentialCall = { tool: call.name, args, result: null, error: null, spent, ok: false }
    calls.push(record)

    // WHAT THE SERVICE SEES, and it is the substituted form — the model's own
    // text with real values where handles were. Everything below judges THAT,
    // which is exactly what an upstream server would.
    const target = call.name === 'http_request' ? String(args.url ?? '') : String(args.command ?? '')
    const grant = world.granted.find((g) => target.includes(g.accepts) || raw.includes(g.handle))

    if (call.name !== 'run_command' && call.name !== 'http_request') {
      record.error = `unknown tool "${call.name}"`
      return { text: record.error, isError: true }
    }
    if (!target.trim()) {
      record.error = call.name === 'http_request' ? 'http_request needs a url.' : 'run_command needs a command.'
      return { text: record.error, isError: true }
    }

    // A HANDLE THAT WAS NEVER GRANTED resolves to nothing — the literal text
    // goes to the service, which rejects it. This is the invented-handle case,
    // and production behaves identically: `resolveHandles` reports `unknown` to
    // the operator and leaves the model's text untouched.
    const written = [...out.matchAll(HANDLE)].map((m) => m[0])
    if (written.length > 0) {
      record.error = `401 Unauthorized — the service rejected the credential. It received the literal text ${written[0]}, which is not a credential this workspace issued to you.`
      return { text: record.error, isError: true }
    }

    if (looksLikePlaceholder(out)) {
      record.error = '401 Unauthorized — the service rejected the credential. It received a placeholder rather than a credential; nothing filled it in.'
      return { text: record.error, isError: true }
    }

    if (!grant || !out.includes(grant.value)) {
      record.error = '401 Unauthorized — no credential was presented.'
      return { text: record.error, isError: true }
    }
    if (!target.includes(grant.accepts)) {
      record.error = `401 Unauthorized — that credential is not valid for ${target.slice(0, 60)}.`
      return { text: record.error, isError: true }
    }

    record.ok = true
    record.result = call.name === 'http_request' ? { status: 200 } : { exitStatus: 0 }
    return { text: call.name === 'http_request' ? '200 OK\n{"ok":true}' : 'Everything up-to-date.\nExit status 0.', isError: false }
  }

  const calledBefore = (a: string, b: string): boolean => {
    const i = calls.findIndex((c) => c.tool === a)
    const j = calls.findIndex((c) => c.tool === b)
    return i !== -1 && j !== -1 && i < j
  }

  return { tools: CREDENTIAL_TOOLS, calls, calledBefore, dispatch, world }
}
