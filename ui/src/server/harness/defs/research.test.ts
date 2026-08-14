import { describe, expect, it } from 'vitest'
import { NO_TOOLS, type EvalContext, type CheckResult } from '@/server/harness/define'
import { toolWireMessage } from '@/server/harness/transport'
import { runHarness, type HarnessDeps, type Transport, type TransportRequest } from '@/server/harness/run'
import {
  citableSource,
  clampQueries,
  queriesFromLines,
  researchQueriesHarness,
  researchSearchHarness,
  researchSynthesisHarness,
  searchTransport,
  toolSearchTransport,
  type QueryPlanInput,
  type SearchSource,
} from '@/server/harness/defs/research'
import type { Capability } from '@/server/harness/capability'
import type { GuardConfig } from '@/server/guardrails'

// Research is the path whose failure is a CONFIDENT, CITED-LOOKING, WRONG
// document, so these cases are about the two ways that happens: a model with no
// web search producing a fluent brief from memory, and a planner whose output
// nobody could read producing no research at all.
//
// Every edge is injected — no database, no gateway, no fleet, and no model
// grading another model anywhere.

const GUARD: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

interface World {
  replies?: string[]
  facts?: Partial<Record<Capability, boolean>>
  /** Capabilities this deployment can REACH through a registered tool. The
   *  floor asks this before refusing, so a model that cannot browse but can
   *  call a search tool is not turned away — see `capability-reach.ts`. */
  reachable?: Partial<Record<Capability, { server: string; tool: string }>>
}

function world(w: World = {}) {
  const requests: TransportRequest[] = []
  const facts: Partial<Record<Capability, { value: boolean; source: 'probe' }>> = {}
  // See judge.test.ts: only a probe fact widens.
  for (const [cap, value] of Object.entries(w.facts ?? {})) facts[cap as Capability] = { value, source: 'probe' }
  const replies = w.replies ?? ['']

  const deps: Partial<HarnessDeps> = {
    resolveModel: async () => ({ model: 'sonar-pro', step: 'role' }),
    routing: async (model) => ({ endpoints: ['perplexity'], upstreamModel: model }),
    personaKeys: async () => [],
    // capability.ts's cardinal rule: only a fact that positively says "no"
    // counts as missing. Unknown is not missing.
    missingCapabilities: async (_key, required) => required.filter((c) => facts[c]?.value === false),
    capabilities: async () => facts,
    reach: async (_keys, wanted) =>
      Object.fromEntries(
        wanted.map((cap) => {
          const supplier = w.reachable?.[cap] ?? null
          return [cap, { capability: cap, reached: supplier !== null, via: supplier ? ('tool' as const) : null, supplier, detail: '' }]
        }),
      ),
    transport: async (req) => {
      requests.push(req)
      return { kind: 'gateway', text: replies[Math.min(requests.length - 1, replies.length - 1)] ?? '', toolNames: [], usage: null, contractDropped: false }
    },
    guardConfig: async () => GUARD,
    guardText: async () => [],
    recordFindings: async () => {},
    recordRun: async () => {},
    now: () => 0,
  }
  return { requests, deps }
}

const PLAN: QueryPlanInput = { question: 'What changed in Postgres 17 logical replication?', max: 3, findingsSoFar: [] }

const plan = (w: ReturnType<typeof world>, input: QueryPlanInput = PLAN) =>
  runHarness(researchQueriesHarness, input, { caller: 'research:test', deps: w.deps })

// ── The planner ──────────────────────────────────────────────────────────────

describe('the research query planner', () => {
  it('reads the object shape json mode forces, and the bare array both', async () => {
    // `response_format: {type:'json_object'}` requires an OBJECT at the top
    // level on most providers, so the prompt asks for {"queries": [...]} — but
    // a model that answers with the bare array the old prompt asked for must
    // keep working, because that is what every install is running today.
    const wrapped = await plan(world({ replies: ['{"queries":["pg17 failover slots","pg17 release notes replication"]}'] }))
    expect(wrapped.value).toEqual(['pg17 failover slots', 'pg17 release notes replication'])

    const bare = await plan(world({ replies: ['["pg17 failover slots","pg17 release notes replication"]'] }))
    expect(bare.value).toEqual(['pg17 failover slots', 'pg17 release notes replication'])
  })

  it('does not mistake a citation marker in the preamble for the query list', async () => {
    // THE BUG THE OLD EXTRACTOR HAD. `/\[[\s\S]*?\]/` is non-greedy, so on a
    // reply that mentions [2] before answering it matched the CITATION MARKER,
    // parsed it as a one-element array and returned ["2"] as the search plan —
    // one wasted search per round, silently. Typed array members mean `[2]`
    // fails validation and the scanner walks on to the real candidate.
    const run = await plan(
      world({ replies: ['Building on finding [2], here are the angles:\n\n{"queries":["logical slot failover standby","pg17 pg_createsubscriber"]}'] }),
    )
    expect(run.value).toEqual(['logical slot failover standby', 'pg17 pg_createsubscriber'])
  })

  it('accepts the record shape a small model reaches for instead of strings', async () => {
    const run = await plan(world({ replies: ['{"queries":[{"query":"pg17 slot failover"},{"q":"pg17 subscriber reseed"}]}'] }))
    expect(run.value).toEqual(['pg17 slot failover', 'pg17 subscriber reseed'])
  })

  it('treats an empty list as a real answer, not a failure', async () => {
    // Saturation ends the research loop. It must never be spelled the same way
    // as "the model produced nothing we could read" — see the comment on
    // `onFailure` in the definition.
    const run = await plan(world({ replies: ['{"queries":[]}'] }))
    expect(run.value).toEqual([])
    expect(run.schemaValid).toBe(true)
  })

  it('repairs once, then reports the contract failure honestly', async () => {
    const w = world({ replies: ['Sure! Here are three good angles to research.', '{"queries":["pg17 slot failover"]}'] })
    const run = await plan(w)
    expect(run.value).toEqual(['pg17 slot failover'])
    expect(run.repairs).toBe(1)

    const dead = world({ replies: ['1. pg17 slot failover\n2. pg17 subscriber reseed'] })
    const gave = await plan(dead)
    // The numbered list is exactly what the line salvage in research.ts reads,
    // and the point is that the harness does NOT quietly succeed on it: the row
    // records a failed contract, and the raw reply is there for the salvage.
    expect(gave.value).toBeNull()
    expect(gave.schemaValid).toBe(false)
    expect(gave.raw).toContain('pg17 slot failover')
  })

  it('still plans for a model that merely follows instructions badly', async () => {
    // `instruction-following: false` is not this harness's floor. A lost plan
    // costs a round, not the run — research.ts salvages a numbered list and
    // falls back to a default query — so a weak model gets its attempt.
    const run = await plan(world({ replies: ['{"queries":["a query long enough"]}'], facts: { 'instruction-following': false } }))
    expect(run.value).toEqual(['a query long enough'])
  })

  it('refuses on a model MEASURED unable to produce JSON', async () => {
    // The same tolerance that used to argue for trying anyway now argues for
    // refusing: a lost plan costs a round, not the run. Under the narrowed fact
    // (see `scoreJson`) this model returned no parseable object on any trial, so
    // the attempt would lose the round AND the call — and the salvage path reads
    // a reply we now know is not coming.
    const run = await plan(world({ replies: ['{"queries":["a query long enough"]}'], facts: { json: false } }))
    expect(run.value).toBeNull()
    expect(run.error).toContain('cannot run harness "research-queries"')
  })
})

describe('the query salvage helpers', () => {
  it('keeps the old line parser verbatim, including its 8-character floor', () => {
    const raw = '# Angles\n1. "pg17 logical slot failover"\n- pg17 pg_createsubscriber\n2. short\n* replication slot survival on promotion'
    expect(queriesFromLines(raw)).toEqual(['pg17 logical slot failover', 'pg17 pg_createsubscriber', 'replication slot survival on promotion'])
  })

  it('dedupes case- and whitespace-insensitively and holds the round budget', () => {
    expect(clampQueries(['PG17  failover', 'pg17 failover', '', '  ', 'second angle', 'third angle'], 2)).toEqual(['PG17  failover', 'second angle'])
  })
})

// ── The search stage ─────────────────────────────────────────────────────────

describe('the research search harness', () => {
  it('refuses a model that is KNOWN not to search, and says which capability is missing', async () => {
    // AUDIT 1.6, the whole reason this harness exists. `resolveRoleModel`
    // checks only that an assignment still routes, so an admin can point
    // Research at a model with no web search — which does not error, it answers
    // from memory in the same confident voice and the report reads as
    // researched. Below the floor the stage refuses instead.
    //
    // It THROWS the refusal rather than returning it, because this harness
    // declares `onFailure: 'throw'` and that policy now covers every failure to
    // produce a value rather than the contract failure alone. `searchStage`'s
    // caller already turns a throw into "(search failed: …)" against that
    // angle, so the sentence reaches the run either way — and an admin who
    // pointed Research at a memory-only model reads which capability is missing
    // instead of a round that quietly returned nothing.
    await expect(
      runHarness(researchSearchHarness, { query: 'node 24 end of life' }, { caller: 'research:test', deps: world({ facts: { search: false } }).deps }),
    ).rejects.toThrow(/known not to support search/)
  })

  it('runs on a model nobody has probed — unknown is not missing', async () => {
    const run = await runHarness(
      researchSearchHarness,
      { query: 'node 24 end of life' },
      { caller: 'research:test', deps: world({ replies: ['Node.js 24 reaches end of life on 2028-04-30.'] }).deps },
    )
    expect(run.value).toContain('2028-04-30')
  })

  it('throws on an unusable reply so the round loop can record that angle as failed', async () => {
    await expect(
      runHarness(researchSearchHarness, { query: 'node 24 end of life' }, { caller: 'research:test', deps: world({ replies: ['   '] }).deps }),
    ).rejects.toThrow(/research-search/)
  })

  it('sends no protocol JSON constraint — sonar answers in prose', async () => {
    const w = world({ replies: ['Node.js 24 reaches end of life on 2028-04-30.'] })
    await runHarness(researchSearchHarness, { query: 'node 24 end of life' }, { caller: 'research:test', deps: w.deps })
    expect(w.requests[0]?.jsonMode).toBe(false)
  })

  it('does NOT refuse a non-browsing model when a registered tool reaches search', async () => {
    // THE CORRECTION. The floor used to ask whether the MODEL browses; a slot an
    // admin assigns is a model running inside Talaria with the tools this org
    // registered. A model measured `search: false` and `tools: true`, with a
    // web-search server in the registry, does the job — and refusing it was a
    // true statement about the weights and a false one about the deployment.
    const run = await runHarness(
      researchSearchHarness,
      { query: 'node 24 end of life' },
      {
        caller: 'research:test',
        deps: world({
          replies: ['Node.js 24 reaches end of life on 2028-04-30.'],
          facts: { search: false, tools: true },
          reachable: { search: { server: 'exa', tool: 'web_search' } },
        }).deps,
      },
    )
    expect(run.value).toContain('2028-04-30')
  })

  it('still refuses when nothing in the install reaches search', async () => {
    // An org with no search server and a memory-only model gets the SAME
    // refusal it always got. Reach widens the question; it does not soften it.
    await expect(
      runHarness(
        researchSearchHarness,
        { query: 'node 24 end of life' },
        { caller: 'research:test', deps: world({ facts: { search: false, tools: true } }).deps },
      ),
    ).rejects.toThrow(/known not to support search/)
  })
})

// ── The tool-driven search path ──────────────────────────────────────────────

describe('what counts as a citable source', () => {
  it('keeps ordinary pages, including ones whose host merely mentions an account', () => {
    for (const url of [
      'https://posthog.com/docs/self-host',
      'https://plausible.io/blog/google-analytics-gdpr',
      'https://en.wikipedia.org/wiki/Web_analytics',
      'https://docs.example.com/accounts/billing-model', // a real doc, under a path with the word in it
      'https://example.com/register-of-members', // not a sign-up page
    ]) {
      expect(citableSource(url), url).toBe(true)
    }
  })

  it('drops pages whose whole purpose is signing in', () => {
    // Straight off a real brief, which recorded 129 "sources" and cited 29 of
    // them — so the Sources section listed these as *(consulted)*.
    for (const url of ['https://myaccount.microsoft.com/login', 'https://signup.live.com/', 'https://www.office.com/login', 'https://accounts.google.com/']) {
      expect(citableSource(url), url).toBe(false)
    }
  })

  it('keeps a URL it cannot parse — that is the walker’s business, not this rule’s', () => {
    expect(citableSource('not a url')).toBe(true)
  })
})

describe('toolSearchTransport', () => {
  const SUPPLIER = { server: 'exa', tool: 'web_search' }
  /** Mirrors `MAX_TOOL_ROUNDS` in research.ts — the budget the closing turn follows. */
  const MAX_ROUNDS = 3

  /** A base transport that answers with one tool call, then with prose. */
  const modelThatSearches = (finalText: string): { base: Transport; seen: TransportRequest[] } => {
    const seen: TransportRequest[] = []
    const base: Transport = async (req) => {
      seen.push(req)
      if (seen.length === 1) {
        return {
          kind: 'gateway',
          text: '',
          toolNames: ['web_search'],
          toolCalls: [{ name: 'web_search', args: JSON.stringify({ query: 'node 24 eol' }) }],
          usage: null,
          contractDropped: false,
        }
      }
      return { kind: 'gateway', text: finalText, toolNames: [], usage: null, contractDropped: false }
    }
    return { base, seen }
  }

  it('answers the call with the id the assistant turn actually carried', async () => {
    // THE BUG, AND IT COST EVERY TOOL-LOOP HARNESS ON ANTHROPIC AND OPENAI. When a
    // provider omits the tool-call id, two files invented their own and disagreed:
    // `toolWireMessage` wrote `call_0` into the assistant turn while this loop
    // wrote the tool NAME into the result. The replay then referred to a call the
    // provider had never been shown —
    //
    //   messages.3.tool.tool_call_id: Field required
    //
    // — and the sweep filed it as "could not reach this model" on an endpoint
    // that was answering everything else. `readToolCalls` assigns the id once
    // now; a fabricated id is fine, two of them is not.
    let turn = 0
    const base: Transport = async () => {
      turn++
      // NO ID, which is the case that broke: the fallback has to be shared.
      if (turn === 1) return { kind: 'gateway', text: '', toolNames: ['web_search'], toolCalls: [{ name: 'web_search', args: '{}' }], usage: null, contractDropped: false }
      return { kind: 'gateway', text: 'answered', toolNames: [], usage: null, contractDropped: false }
    }
    const seen: TransportRequest[] = []
    const traced: Transport = async (req) => {
      seen.push(req)
      return base(req)
    }
    const transport = toolSearchTransport('run-1', [], SUPPLIER, { base: traced, callTool: async () => ({ text: 'results', structured: null }) })
    await transport({ model: 'sonnet', messages: [{ role: 'user', content: 'q' }], jsonMode: false, caller: 't' })

    // ASSERTED ON THE WIRE FORM, which is where the two halves have to agree —
    // `toolWireMessage` is what fills the assistant turn's `id`, so comparing the
    // in-memory `Message` objects would compare one side before it is rendered.
    const replay = seen[1]!.messages
    const assistant = toolWireMessage(replay.find((m) => m.role === 'assistant' && m.toolCalls?.length)!) as { tool_calls: Array<{ id: string }> }
    const result = toolWireMessage(replay.find((m) => m.role === 'tool')!) as { tool_call_id: string }
    expect(result.tool_call_id).toBeTruthy()
    expect(result.tool_call_id).toBe(assistant.tool_calls[0]?.id)
  })

  it('replays tool calls on the TOOL CHANNEL, never as prose the model can imitate', async () => {
    // THE FAILURE THIS PREVENTS, verbatim from a live sweep. The loop used to
    // push `Called web_search({...})` as ASSISTANT TEXT and the results as a
    // USER turn. deepseek-v4-flash read that transcript, concluded that is what
    // an assistant turn looks like here, and returned
    //   Called web_search({"query":"AC-2 Account Management NIST 800-53 Rev 5 ..."})
    // as its FINAL ANSWER on fixture after fixture. Same failure as the dry-run
    // sandbox (see `Message.toolCalls` in define.ts): changing the wording moves
    // the imitation, and only the channel ends it.
    const seen: TransportRequest[] = []
    const base: Transport = async (req) => {
      seen.push(req)
      if (seen.length === 1) {
        return {
          kind: 'gateway',
          text: '',
          toolNames: ['web_search'],
          toolCalls: [{ name: 'web_search', args: JSON.stringify({ query: 'node 24 eol' }), id: 'call_7' }],
          usage: null,
          contractDropped: false,
        }
      }
      return { kind: 'gateway', text: 'Node.js 24 reaches end of life on 2028-04-30.', toolNames: [], usage: null, contractDropped: false }
    }
    const transport = toolSearchTransport('run-1', [], SUPPLIER, { base, callTool: async () => ({ text: 'EOL 2028-04-30', structured: null }) })
    await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'node 24 eol' }], jsonMode: false, caller: 't' })

    const replay = seen[1]!.messages
    // NOTHING NARRATES A CALL IN PROSE — that is the string models copy.
    expect(replay.some((m) => /Called web_search\(/.test(m.content))).toBe(false)
    // The call rides the assistant turn's own channel...
    const assistant = replay.find((m) => m.role === 'assistant' && m.toolCalls?.length)
    expect(assistant?.toolCalls?.[0]?.name).toBe('web_search')
    // ...and the result is a `tool` message answering it by id, not a user turn
    // pretending to be a person reading search results aloud.
    const result = replay.find((m) => m.role === 'tool')
    expect(result?.toolCallId).toBe('call_7')
    expect(result?.content).toContain('EOL 2028-04-30')
  })

  it('spends another round on an EMPTY turn rather than concluding from it', async () => {
    // THE MOST MISLEADING ERROR IN THE SUITE, now that it is gone. A model that
    // returned no text and no tool call used to break the loop and throw
    // "answered the search query without calling web_search" — accusing it of
    // answering from memory, which is the opposite of what happened — and
    // `runHarness` wrapped that as "could not reach", which reads as a
    // connection error. Three wrong statements about one empty reply, and the
    // reason five research-search cases looked like a network fault.
    let turn = 0
    const base: Transport = async () => {
      turn++
      // Nothing at all on the first turn; then it works normally.
      if (turn === 1) return { kind: 'gateway', text: '', toolNames: [], toolCalls: [], usage: null, contractDropped: false }
      if (turn === 2) {
        return { kind: 'gateway', text: '', toolNames: ['web_search'], toolCalls: [{ name: 'web_search', args: '{}', id: 'c1' }], usage: null, contractDropped: false }
      }
      return { kind: 'gateway', text: 'Node.js 24 reaches end of life on 2028-04-30.', toolNames: [], usage: null, contractDropped: false }
    }
    const transport = toolSearchTransport('run-1', [], SUPPLIER, { base, callTool: async () => ({ text: 'EOL 2028-04-30', structured: null }) })
    const reply = await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'node 24 eol' }], jsonMode: false, caller: 't' })
    expect(reply.text).toContain('2028-04-30')
  })

  it('says the DEPLOYMENT failed when every round comes back empty', async () => {
    // The other half: a model that never answers and never searches has told us
    // nothing about its research, and the sentence has to say so rather than
    // blame its judgement — that is the difference between a red cell an admin
    // should act on and one they should ignore.
    const base: Transport = async () => ({ kind: 'gateway', text: '', toolNames: [], toolCalls: [], usage: null, contractDropped: false })
    const transport = toolSearchTransport('run-1', [], SUPPLIER, { base, callTool: async () => ({ text: '', structured: null }) })
    await expect(
      transport({ model: 'deepseek', messages: [{ role: 'user', content: 'node 24 eol' }], jsonMode: false, caller: 't' }),
    ).rejects.toThrow(/empty turn every round.*This is the deployment, not the model/s)
  })

  it('ASKS FOR AN ANSWER when the round budget runs out mid-search', async () => {
    // THE BUG THIS PINS. A model still searching when the rounds ran out had its
    // INTERSTITIAL turn recorded as its finding. A live sweep of
    // deepseek-v4-flash failed nine of nine research-search fixtures on text
    // like "The searches so far have returned mostly generic EU pages ... Let me
    // try more targeted queries" — the model had gathered the evidence and was
    // never asked to use it. Three more scored "the model returned nothing",
    // which is the same bug when the preamble is empty.
    const seen: TransportRequest[] = []
    const base: Transport = async (req) => {
      seen.push(req)
      // Never stops asking: every turn is another tool call, so the loop can
      // only ever exit on the budget.
      if (seen.length <= MAX_ROUNDS) {
        return {
          kind: 'gateway',
          text: 'Let me try more targeted queries.',
          toolNames: ['web_search'],
          toolCalls: [{ name: 'web_search', args: JSON.stringify({ query: 'ai act gpai' }) }],
          usage: null,
          contractDropped: false,
        }
      }
      return { kind: 'gateway', text: 'The GPAI obligations apply from 2025-08-02.', toolNames: [], usage: null, contractDropped: false }
    }
    const sink: SearchSource[] = []
    const transport = toolSearchTransport('run-1', sink, SUPPLIER, {
      base,
      callTool: async () => ({ text: 'GPAI obligations apply from 2 August 2025.', structured: { results: [{ url: 'https://eur-lex.europa.eu/ai-act', title: 'AI Act' }] } }),
    })

    const reply = await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'when do EU AI Act GPAI rules apply' }], jsonMode: false, caller: 't' })

    // The answer, not the preamble.
    expect(reply.text).toContain('2025-08-02')
    expect(reply.text).not.toContain('more targeted queries')
    // NO TOOLS ON THE CLOSING TURN: leaving them on is an invitation to spend it
    // on a fourth search and arrive back with nothing again.
    const closing = seen.at(-1)
    expect(closing?.toolDefs ?? []).toEqual([])
    expect(closing?.messages.at(-1)?.content).toContain('do not search again')
  })

  it('never records a tool-calling turn\u2019s prose as the finding', async () => {
    // The narrower half of the same rule: even when the model DOES stop on its
    // own, the text has to come from the turn that stopped — not from an earlier
    // turn that was still working.
    const seen: TransportRequest[] = []
    const base: Transport = async (req) => {
      seen.push(req)
      if (seen.length === 1) {
        return {
          kind: 'gateway',
          text: 'First I will check the release schedule.',
          toolNames: ['web_search'],
          toolCalls: [{ name: 'web_search', args: '{}' }],
          usage: null,
          contractDropped: false,
        }
      }
      return { kind: 'gateway', text: 'Node.js 24 reaches end of life on 2028-04-30.', toolNames: [], usage: null, contractDropped: false }
    }
    const transport = toolSearchTransport('run-1', [], SUPPLIER, { base, callTool: async () => ({ text: 'EOL 2028-04-30', structured: null }) })
    const reply = await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'node 24 eol' }], jsonMode: false, caller: 't' })
    expect(reply.text).toBe('Node.js 24 reaches end of life on 2028-04-30.')
  })

  it('calls the tool, harvests its sources, and answers from what came back', async () => {
    const sink: SearchSource[] = []
    const { base, seen } = modelThatSearches('Node.js 24 reaches end of life on 2028-04-30.')
    const transport = toolSearchTransport('run-1', sink, SUPPLIER, {
      base,
      callTool: async () => ({
        text: 'Node 24 EOL is 2028-04-30.',
        structured: { results: [{ url: 'https://github.com/nodejs/Release', title: 'nodejs/Release', snippet: 'Node 24 …' }] },
      }),
    })

    const reply = await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'node 24 end of life' }], jsonMode: false, caller: 't' })

    expect(reply.text).toContain('2028-04-30')
    // The sources are the product: without them the synthesis stage has nothing
    // to cite and `ungrounded_ref` has nothing to ground against.
    expect(sink).toEqual([{ url: 'https://github.com/nodejs/Release', title: 'nodejs/Release', snippet: 'Node 24 …' }])
    // The tool definition was actually offered, and the native "you ARE a search
    // engine" framing was replaced rather than stacked on top of the new one.
    expect(seen[0]?.toolDefs?.[0]?.name).toBe('web_search')
    expect(seen[0]?.messages.filter((m) => m.role === 'system')).toHaveLength(1)
    expect(seen[0]?.messages[0]?.content).toContain('web_search')
  })

  it('FAILS a model that answered without searching, rather than passing off memory as research', async () => {
    // The precise failure the search floor exists to prevent. Prose from
    // training data is indistinguishable from prose from the web by looking at
    // it, so the distinction has to be made here, where it is still visible.
    const sink: SearchSource[] = []
    const transport = toolSearchTransport('run-1', sink, SUPPLIER, {
      base: async () => ({ kind: 'gateway', text: 'Node 24 reaches EOL in April 2028, I believe.', toolNames: [], usage: null, contractDropped: false }),
      callTool: async () => ({ text: '', structured: null }),
    })

    await expect(transport({ model: 'deepseek', messages: [{ role: 'user', content: 'q' }], jsonMode: false, caller: 't' })).rejects.toThrow(/without calling/)
  })

  it('finds sources in whatever envelope the server used', async () => {
    // MCP search servers disagree about the wrapper and agree about the leaf,
    // which is an object with a URL on it.
    const sink: SearchSource[] = []
    const { base } = modelThatSearches('done')
    const transport = toolSearchTransport('run-1', sink, SUPPLIER, {
      base,
      callTool: async () => ({ text: '', structured: { data: { hits: [{ link: 'https://a.example/x', name: 'A' }, { href: 'https://b.example/y' }] } } }),
    })
    await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'q' }], jsonMode: false, caller: 't' })
    expect(sink.map((s) => s.url)).toEqual(['https://a.example/x', 'https://b.example/y'])
  })

  it('keeps going when the tool itself fails, and says so in the transcript', async () => {
    // One dead tool call costs one angle, not the run — the same posture the
    // round loop takes around the whole stage.
    const sink: SearchSource[] = []
    const { base, seen } = modelThatSearches('The search tool was unavailable, so I could not verify this.')
    const transport = toolSearchTransport('run-1', sink, SUPPLIER, {
      base,
      callTool: async () => {
        throw new Error('upstream 503')
      },
    })
    const reply = await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'q' }], jsonMode: false, caller: 't' })
    expect(reply.text).toContain('could not verify')
    // ANYWHERE IN THE REPLAY, not at a fixed position: the synthesis rules now
    // arrive with the first results, so the tool's failure is no longer the last
    // message. What the assertion is about is that the model SEES the failure.
    expect(seen[1]?.messages.map((m) => m.content).join('\n')).toContain('upstream 503')
    expect(sink).toEqual([])
  })

  it('supplies the stage’s own question when the model calls the tool with junk arguments', async () => {
    const sink: SearchSource[] = []
    let got: Record<string, unknown> = {}
    const seen: TransportRequest[] = []
    const base: Transport = async (req) => {
      seen.push(req)
      if (seen.length === 1) {
        return { kind: 'gateway', text: '', toolNames: [], toolCalls: [{ name: 'web_search', args: 'not json at all' }], usage: null, contractDropped: false }
      }
      return { kind: 'gateway', text: 'ok', toolNames: [], usage: null, contractDropped: false }
    }
    const transport = toolSearchTransport('run-1', sink, SUPPLIER, {
      base,
      callTool: async (_s, _t, args) => {
        got = args
        return { text: '', structured: null }
      },
    })
    await transport({ model: 'deepseek', messages: [{ role: 'user', content: 'node 24 end of life' }], jsonMode: false, caller: 't' })
    expect(got.query).toBe('node 24 end of life')
  })
})

// ── The synthesis stage ──────────────────────────────────────────────────────

const SYNTH = {
  question: 'When does Node.js 24 reach end of life?',
  mode: 'recon' as const,
  sources: [{ idx: 1, url: 'https://github.com/nodejs/Release', title: 'nodejs/Release' }],
  findings: ['### Query: node 24 end of life\nThe schedule gives Node.js 24 an end-of-life date of 2028-04-30 [1].'],
  searchFailed: false,
}

describe('the research synthesis harness', () => {
  it('shows the model only the source numbers it is allowed to cite', async () => {
    const w = world({ replies: ['# Node.js 24 end of life\n\nNode.js 24 reaches end of life on 2028-04-30 [1].'] })
    const run = await runHarness(researchSynthesisHarness, SYNTH, { caller: 'research:test', deps: w.deps })
    expect(run.value).toContain('[1]')
    const prompt = w.requests[0]?.messages.map((m) => m.content).join('\n') ?? ''
    expect(prompt).toContain('[1] nodejs/Release')
    expect(prompt).not.toContain('[2]')
  })

  it('throws rather than saving an empty report over paid-for searches', async () => {
    // Before the port an empty reply produced an artifact with a generated
    // title, no body and a run marked done.
    await expect(runHarness(researchSynthesisHarness, SYNTH, { caller: 'research:test', deps: world({ replies: [''] }).deps })).rejects.toThrow(
      /research-synthesis/,
    )
  })

  it('widens only on a model known to hold the findings in view', async () => {
    const narrow = world({ replies: ['# Title\n\nBody [1].'] })
    await runHarness(researchSynthesisHarness, SYNTH, { caller: 'research:test', deps: narrow.deps })
    expect(narrow.requests[0]?.messages[0]?.content).not.toContain('Reconcile them')

    const wide = world({ replies: ['# Title\n\nBody [1].'], facts: { 'long-context': true, 'instruction-following': true } })
    await runHarness(researchSynthesisHarness, SYNTH, { caller: 'research:test', deps: wide.deps })
    expect(wide.requests[0]?.messages[0]?.content).toContain('Reconcile them')
  })
})

// ── The rule this whole path exists to exercise ──────────────────────────────
//
// `ungrounded_ref` self-skips on all 23 harnesses by construction — the runner
// derives its tool record from the messages IT sent and a harness turn carries
// no tool results — so before `HarnessDefinition.ground` existed, the one
// document in Talaria whose defining failure mode is a fabricated citation was
// the one document nothing checked. `server/research.ts` worked around it with
// a hand-built guard pass; these cases assert the fold is real, on the REAL
// harness rather than on a synthetic one (run.test.ts covers the hook itself).

/** Same shape as `world`, plus a policed host and a findings sink, because what
 *  is being asserted here is that a rule FIRED and was RECORDED. `kind: 'fleet'`
 *  is production's shape: the synthesis runs on the requesting agent's persona,
 *  whose branch is owed `{ results: false }` — so a firing rule proves `ground`
 *  overrode it, which is the load-bearing half. */
function grounded(reply: string) {
  const recorded: Array<{ check: string }> = []
  const config: GuardConfig = { ...GUARD, policedHosts: ['talaria.internal'] }
  const deps: Partial<HarnessDeps> = {
    resolveModel: async () => ({ model: 'dex-developer', step: 'utility' }),
    routing: async (model) => ({ endpoints: ['fleet'], upstreamModel: model }),
    personaKeys: async () => [],
    missingCapabilities: async () => [],
    capabilities: async () => ({}),
    transport: async () => ({ kind: 'fleet', text: reply, toolNames: [], usage: null, contractDropped: false }),
    guardConfig: async () => config,
    guardText: async () => [],
    recordFindings: async (f) => {
      recorded.push(...f)
    },
    recordRun: async () => {},
    now: () => 0,
  }
  return { recorded, deps }
}

const INVENTED = 'https://talaria.internal/tickets/PLAT-9001'
const REAL = 'https://github.com/nodejs/Release'

describe('the research report is grounded against its own search hits', () => {
  it('FIRES ungrounded_ref on an internal link no search result contained', async () => {
    const w = grounded(`# Node.js 24 end of life\n\nNode.js 24 goes EOL on 2028-04-30 [1]. Tracked at ${INVENTED}.`)
    const run = await runHarness(researchSynthesisHarness, SYNTH, { caller: 'research:test', deps: w.deps })
    expect(run.findings.map((f) => f.check)).toContain('ungrounded_ref')
    expect(w.recorded.map((f) => f.check)).toContain('ungrounded_ref')
  })

  it('does NOT fire on a link the findings actually carry', async () => {
    // Same document shape, one URL swapped for one the search stage returned.
    // Without this case the one above would pass on a rule that flags every URL.
    const w = grounded(`# Node.js 24 end of life\n\nNode.js 24 goes EOL on 2028-04-30 [1]. Source: ${REAL}.`)
    const run = await runHarness(researchSynthesisHarness, SYNTH, { caller: 'research:test', deps: w.deps })
    expect(run.findings.map((f) => f.check)).not.toContain('ungrounded_ref')
  })

  it('grounds against the FULL findings, not the truncated copy the prompt carried', async () => {
    // `synthPrompt` caps the findings at NOTES_CAP; `ground` deliberately does
    // not. A citation that fell off the end of the prompt is still a citation
    // the pipeline retrieved, and flagging it would be a false positive.
    const filler = 'x'.repeat(90_000) // longer than NOTES_CAP, so the line below falls off the prompt
    const input = { ...SYNTH, findings: [filler, `### Query: ticket\nThe rollout is tracked at ${INVENTED}.`] }
    const w = grounded(`# Rollout\n\nThe rollout is tracked at ${INVENTED} [1].`)
    const run = await runHarness(researchSynthesisHarness, input, { caller: 'research:test', deps: w.deps })
    expect(run.findings.map((f) => f.check)).not.toContain('ungrounded_ref')
    expect(w.recorded).toEqual([])
  })

  it('is the only transport left outside transport.ts, and it keeps that file’s rule', async () => {
    // `searchTransport` stays beside its harness because the SOURCES are the
    // product and `completeViaGateway` throws the provider's `search_results` /
    // `citations` away. That makes it the one place a request field could be
    // dropped instead of refused — the exact failure the five deleted shims kept
    // making — so it throws on `tools: 'own'` the way `gatewayTransport` and
    // `gatewayStream` do. It never touches the network to find out.
    const t = searchTransport('run-1', [])
    await expect(
      t({ model: 'sonar-pro', messages: [{ role: 'user', content: 'q' }], jsonMode: false, tools: 'own', caller: 'research:run-1' }),
    ).rejects.toThrow(/ORG GATEWAY/)
  })

  it('declares the rule rather than leaving it to a caller', async () => {
    // The regression this guards: `server/research.ts` used to run this one rule
    // itself, so deleting it from the harness list would have been invisible.
    expect(researchSynthesisHarness.guard?.rules).toContain('ungrounded_ref')
    expect(researchSynthesisHarness.ground).toBeTypeOf('function')
    expect(researchSynthesisHarness.ground?.(SYNTH)).toMatchObject({ tools: ['research_search'], errored: false })
  })
})

// ── The eval fixtures ────────────────────────────────────────────────────────
// The fitness suite replays these against a candidate model. A check that
// passes everything scores every model identically and is worse than no eval,
// so each one is exercised against a plausible BAD answer as well as a good one.

const checkOf = <I, O>(evals: Array<{ name: string; input: I; check: (v: O, ctx: EvalContext) => CheckResult }> | undefined, name: string) => {
  const found = evals?.find((e) => e.name.startsWith(name))
  if (!found) throw new Error(`no eval fixture starting "${name}"`)
  // These fixtures are all single-shot, so the context is the empty one every
  // non-dry-run harness receives. Bound here rather than at each of the fifteen
  // call sites below.
  return (v: O) => found.check(v, NO_TOOLS)
}

describe('the research eval fixtures', () => {
  it('fails a plan that only rewords the question', () => {
    const check = checkOf(researchQueriesHarness.evals, 'plans distinct angles')
    expect(
      check([
        'Which EU rules apply to open-weight foundation models released in 2026',
        'EU rules for open-weight foundation models publisher requirements 2026',
      ]),
    ).toMatch(/restates rather than researches|verbatim/)
    expect(check(['EU AI Act GPAI transparency obligations Article 53', 'open weight model exemption AI Act recital', 'AI Act 2026 enforcement timeline'])).toBeNull()
  })

  it('fails a plan that re-searches a question the findings already settled', () => {
    const check = checkOf(researchQueriesHarness.evals, 'gap round')
    expect(check(['vendor SOC 2 Type II report exceptions'])).toMatch(/already settled/)
    expect(check(['vendor subprocessor non-EEA transfer mechanism SCCs'])).toBeNull()
  })

  it('fails a search answer that declines to browse', () => {
    const check = checkOf(researchSearchHarness.evals, 'answers a time-sensitive')
    expect(check('I do not have access to real-time information, so I cannot tell you the current LTS version.')).toMatch(/no live search/)
    expect(check(`Node.js 24 entered active LTS on 2025-10-28 and reaches end of life on 2028-04-30. ${'Detail. '.repeat(30)}`)).toBeNull()
  })

  it('fails a document that cites a source the registry does not have', () => {
    const check = checkOf(researchSynthesisHarness.evals, 'a thin registry')
    expect(check('# Node 24\n\nIt ends on 2028-04-30 [1], per the release working group [2].')).toMatch(/only source \[1\] exists/)
    expect(check('# Node 24\n\nIt ends on 2028-04-30 [1].')).toBeNull()
  })

  it('fails a document that silently picks a winner between contradictory sources', () => {
    const check = checkOf(researchSynthesisHarness.evals, 'contradictory findings')
    expect(check('# Acme headcount\n\nAcme employs 4,200 people [1].')).toMatch(/reported only 4,200/)
    expect(check('# Acme headcount\n\nThe 2025 annual report says 4,200 [1]; a March 2026 post says over 5,000 [2]. The report is the audited figure.')).toBeNull()
  })
})

describe('citation markers past the two-digit line', () => {
  // THE SHAPE CHANGE THAT EXPOSED THIS. A sonar run answers with a handful of
  // pre-ranked sources, so a registry never got near [99] and `\d{1,2}` was
  // invisible. Research is model-agnostic now: an expedition is up to twelve
  // queries against a web-search tool, each returning a page of results, and the
  // registry numbers every distinct URL. Three figures is ordinary.
  const fixture = (name: string) => {
    const f = (researchSynthesisHarness.evals ?? []).find((e) => e.name.startsWith(name))
    if (!f) throw new Error(`no fixture starting "${name}"`)
    return f
  }
  const check = (v: string) => fixture('a large registry').check(v, NO_TOOLS)

  it('accepts a three-digit citation the registry actually has', () => {
    expect(check('# Logical replication at volume\n\nA stalled subscriber pins WAL until it catches up [104].')).toBeNull()
  })

  it('does not read a report citing ONLY three-digit sources as citing nothing', () => {
    // The old regex matched none of them, so `cited.length === 0` and the report
    // failed for having no citations while being fully cited.
    const out = check('# Logical replication at volume\n\nSlots do not follow a failover [118], and WAL is pinned meanwhile [104].')
    expect(out).toBeNull()
  })

  it('still catches an INVENTED three-digit citation, which used to pass unseen', () => {
    // The registry carries 120. [150] is invention, and two-digit matching could
    // not see it at all.
    expect(check('# Logical replication at volume\n\nThroughput collapses above 40k writes per second [150].')).toContain('[150]')
  })

  it('leaves a four-digit number alone, because that is a year and not a source', () => {
    // Matching it would strip dates out of reports.
    expect(check('# Logical replication at volume\n\nThe behaviour has been stable since [2024] per source [7].')).toBeNull()
  })
})
