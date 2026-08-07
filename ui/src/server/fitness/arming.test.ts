import { beforeEach, describe, expect, it, vi } from 'vitest'

// THE ARMED PATH, END TO END, WITH NOTHING FAKED BETWEEN THE PIECES.
//
// Every other suite covers one hop: `transport.test.ts` proves the gateway puts
// tool definitions on the wire, `probes.test.ts` proves `scoreToolSelect` grades
// four calls, `capability.test.ts` proves a fact round-trips, `run.test.ts`
// proves the widening gate reads `source`. Each of those stubs the hop on either
// side of it, so until this file none of them answered the question the feature
// is actually about: does a tool call a provider reported become a widened Inbox
// surface, through the real code, in one motion?
//
// It matters because the fitness workflow's own reconcile pass reported the
// feature UNARMED — `TransportRequest` had no slot for a tool definition, so
// `tool-select` skipped forever, so no `value: true` fact existed anywhere in
// Talaria, so `runHarness`'s widening branch had never executed in production on
// any install. A chain that long fails silently at any link, and every link is
// tested. So this runs the chain: a transport that reports real tool calls ->
// runProbes -> the REAL recordCapability -> the REAL getCapabilities ->
// runHarness's widening gate -> the action allowlist the Inbox validates
// against. Only the transport and `routingFor` are scripted.
const { store } = vi.hoisted(() => ({ store: new Map<string, unknown>() }))
vi.mock('../audit', () => ({
  getSetting: <T>(key: string, fallback: T): Promise<T> => Promise.resolve(store.has(key) ? (store.get(key) as T) : fallback),
  setSetting: (key: string, value: unknown): Promise<void> => {
    store.set(key, structuredClone(value))
    return Promise.resolve()
  },
}))
vi.mock('@/server/audit', () => ({
  getSetting: <T>(key: string, fallback: T): Promise<T> => Promise.resolve(store.has(key) ? (store.get(key) as T) : fallback),
  setSetting: (key: string, value: unknown): Promise<void> => {
    store.set(key, structuredClone(value))
    return Promise.resolve()
  },
  logAudit: () => Promise.resolve(),
}))
vi.mock('@/server/model-access', () => ({ gatewayModelsFor: async () => [], memberModelAllowlist: async () => [], modelAllowedFor: () => true }))
vi.mock('@/server/fleet-agents', () => ({ routedModelFor: (m: string) => m }))
vi.mock('../llm-gateway', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  routingFor: () => Promise.resolve({ endpoints: [{ name: 'pl-main' }], upstreamModel: 'qwen3-14b' }),
}))

import { runProbes, runnerToolAsk } from './probes'
import { capabilityKey, getCapabilities, recordCapability } from '../harness/capability'
import { runHarness, type Transport } from '../harness/run'
import { allowedFocusActionIds, inboxCommandHarness, type FocusCommandInput, type FocusHarnessItem } from '../harness/defs/inbox-focus'

const CORRECT: Record<string, string> = {
  'tool-select:weather': 'get_weather',
  'tool-select:email': 'send_email',
  'tool-select:currency': 'convert_currency',
  'tool-select:ticket': 'create_ticket',
}

/** A transport as a GATEWAY would answer it: text plus the tool calls the
 *  provider reported. Nothing above the transport is faked. */
const toolTransport: Transport = async (req) => {
  const id = req.caller?.replace('fitness:probe:', '') ?? ''
  const defs = req.toolDefs ?? []
  if (!defs.length) return { kind: 'gateway', text: 'ok', toolNames: [], usage: null, contractDropped: false }
  const want = CORRECT[id] ?? 'get_weather'
  const calls = [{ name: want, args: '{"city":"Lisbon"}' }]
  return { kind: 'gateway', text: '', toolNames: calls.map((c) => c.name), toolCalls: calls, usage: null, contractDropped: false }
}

const KEY = capabilityKey('pl-main', 'qwen3-14b')

beforeEach(() => store.clear())

describe('END TO END: a tool call the transport reported becomes a widened Inbox surface', () => {
  it('writes a probe fact and the runner widens on it', async () => {
    const report = await runProbes('qwen3-14b', {
      ids: ['tool-select'],
      deps: {
        askWithTools: runnerToolAsk('qwen3-14b', toolTransport),
        offersToolDefinitions: () => Promise.resolve(true),
        record: recordCapability,
      },
    })
    expect(report.wrote).toBe(1)

    // The fact is in the real store, with the real provenance.
    const facts = await getCapabilities(KEY)
    expect(facts['tool-select']).toMatchObject({ value: true, source: 'probe', score: 1 })

    // ...and the runner widens on it, reading the same store.
    await recordCapability(KEY, 'instruction-following', { value: true, source: 'probe', at: new Date().toISOString(), score: 1 })
    const result = await runHarness(
      inboxCommandHarness,
      INPUT('what do you make of this?'),
      {
        caller: 'test:reconcile',
        model: 'qwen3-14b',
        deps: {
          routing: async (m: string) => ({ endpoints: ['pl-main'], upstreamModel: m }),
          transport: async () => ({ kind: 'gateway' as const, text: '{"message":"Ready.","actionId":"approve_task"}', toolNames: [], usage: null, contractDropped: false }),
          guardConfig: async () => ({ mode: 'off' as const, checks: {}, minConfidence: 1, policedHosts: [], coach: false }),
          guardText: async () => [],
          recordFindings: async () => {},
          recordRun: async () => {},
        },
      },
    )
    expect(result.widened).toBe(true)
    expect(allowedFocusActionIds(INPUT('x'), true)).toContain('approve_task')
  })

  it('REFUSES a declared fact and honors only the probe one', async () => {
    const at = new Date().toISOString()
    await recordCapability(KEY, 'tool-select', { value: true, source: 'declared', at })
    await recordCapability(KEY, 'instruction-following', { value: true, source: 'declared', at })
    const wide = async (): Promise<boolean> =>
      (
        await runHarness(inboxCommandHarness, INPUT('x'), {
          caller: 'test:reconcile',
          model: 'qwen3-14b',
          deps: {
            routing: async (m: string) => ({ endpoints: ['pl-main'], upstreamModel: m }),
            transport: async () => ({ kind: 'gateway' as const, text: '{"message":"Ready.","actionId":"approve_task"}', toolNames: [], usage: null, contractDropped: false }),
            guardConfig: async () => ({ mode: 'off' as const, checks: {}, minConfidence: 1, policedHosts: [], coach: false }),
            guardText: async () => [],
            recordFindings: async () => {},
            recordRun: async () => {},
          },
        })
      ).widened
    expect(await wide()).toBe(false)

    await recordCapability(KEY, 'tool-select', { value: true, source: 'probe', at, score: 1 })
    await recordCapability(KEY, 'instruction-following', { value: true, source: 'probe', at, score: 1 })
    expect(await wide()).toBe(true)
  })
})

/** The command harness's input, with the fields the runner and the allowlist
 *  both read. Built once so the run and the allowlist assertion cannot drift. */
const INPUT = (instruction: string): FocusCommandInput => ({
  item: ITEM,
  instruction,
  history: [],
  mode: 'normal',
  // No regex matches this instruction, so the ONLY thing that can put
  // `approve_task` in front of the model is the probe fact.
  deterministicActionId: null,
  role: 'orchestrator',
  specialist: null,
})

const ITEM: FocusHarnessItem = {
  key: 'task:t1',
  question: 'What next?',
  sourceHref: '/app/boards/b1?task=t1',
  evidence: [],
  metadata: {},
  actions: [
    { id: 'approve_task', label: 'Approve', risk: 'safe', confirmationRequired: false, reversible: true },
    { id: 'request_changes', label: 'Request changes', risk: 'safe', confirmationRequired: false, reversible: true },
  ],
}
