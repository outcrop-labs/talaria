import { describe, expect, it } from 'vitest'
import {
  DEFAULT_TOOL_TIMEOUT_SECS,
  checkAppMcpTools,
  catalogOf,
  isToolResult,
  isUnresolvedHandle,
  runAppTools,
  shapeToolAnswer,
  timeoutSecsOf,
  toolResult,
} from './app-mcp-call'
import { memoryStore } from './app-store-memory'
import type { AppMcpTool } from '@/sdk/server'

// THE QUESTION THIS FILE PINS DOWN: what one app tool call is, with no host
// around it — what the handler is told, what it may return and how that
// reaches the agent, and when the call is cut. The gateway route and
// @talaria/sdk/testing both run exactly this code.

const tool = (over: Partial<AppMcpTool> = {}): AppMcpTool => ({
  name: 'crm_find',
  description: 'Find a contact.',
  handler: async () => 'ok',
  ...over,
})
const facts = { caller: { name: 'crm-agent', id: 'def-1', proven: true }, actingUserId: null, store: memoryStore() }
const callOf = (name: string, args: Record<string, unknown> = {}) => ({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name, arguments: args } })

describe('what a handler returns, as the agent receives it', () => {
  it('a string is one text block; any other value its pretty JSON — as before', () => {
    expect(shapeToolAnswer(tool(), 'hello')).toEqual({ text: 'hello' })
    expect(shapeToolAnswer(tool(), { a: 1 })).toEqual({ text: '{\n  "a": 1\n}' })
    expect(shapeToolAnswer(tool(), undefined)).toEqual({ text: 'null' })
  })

  it('toolResult passes content, structuredContent and isError through unchanged', () => {
    const r = toolResult({
      content: [
        { type: 'text', text: 'two contacts' },
        { type: 'image', data: 'aGk=', mimeType: 'image/png' },
        { type: 'resource_link', uri: 'https://crm.example/c/1', name: 'Ada' },
      ],
      structuredContent: { ids: ['1', '2'] },
    })
    expect(isToolResult(r)).toBe(true)
    expect(isToolResult({ content: [] })).toBe(false)
    expect(shapeToolAnswer(tool(), r)).toEqual({
      result: {
        content: [
          { type: 'text', text: 'two contacts' },
          { type: 'image', data: 'aGk=', mimeType: 'image/png' },
          { type: 'resource_link', uri: 'https://crm.example/c/1', name: 'Ada' },
        ],
        structuredContent: { ids: ['1', '2'] },
      },
    })
    expect(shapeToolAnswer(tool(), toolResult({ content: [{ type: 'text', text: 'no such contact' }], isError: true }))).toEqual({
      result: { content: [{ type: 'text', text: 'no such contact' }], isError: true },
    })
  })

  it('structuredContent with no content gets its JSON as a text block, for clients that read only text', () => {
    expect(shapeToolAnswer(tool(), toolResult({ structuredContent: { n: 2 } }))).toEqual({
      result: { content: [{ type: 'text', text: '{"n":2}' }], structuredContent: { n: 2 } },
    })
  })

  it('a malformed toolResult is the server’s error (-32603), naming the tool and the block', () => {
    const bad = [
      [toolResult({ content: 'x' as never }), 'whose content is not an array'],
      [toolResult({ content: [{ type: 'video' } as never] }), 'content[0] has type "video"'],
      [toolResult({ content: [{ type: 'text' } as never] }), 'content[0] is a text block without a string `text`'],
      [toolResult({ structuredContent: [1, 2] as never }), 'whose structuredContent is not a JSON object'],
    ] as const
    for (const [out, why] of bad) {
      const a = shapeToolAnswer(tool(), out) as { error: { code: number; message: string } }
      expect(a.error.code).toBe(-32603)
      expect(a.error.message).toContain('tool "crm_find" returned a toolResult')
      expect(a.error.message).toContain(why)
    }
  })

  it('a tool with an outputSchema answers structured, and only what the schema admits', () => {
    const counted = tool({ outputSchema: { type: 'object', properties: { count: { type: 'integer' } }, required: ['count'] } })
    expect(shapeToolAnswer(counted, { count: 3 })).toEqual({ result: { content: [{ type: 'text', text: '{"count":3}' }], structuredContent: { count: 3 } } })
    expect(shapeToolAnswer(counted, toolResult({ content: [{ type: 'text', text: 'three' }], structuredContent: { count: 3 } }))).toEqual({
      result: { content: [{ type: 'text', text: 'three' }], structuredContent: { count: 3 } },
    })
    const refused = shapeToolAnswer(counted, { count: 'three' }) as { error: { message: string } }
    expect(refused.error.message).toMatch(/^tool "crm_find" returned structuredContent its outputSchema refuses:\n/)
    expect((shapeToolAnswer(counted, 'three') as { error: { message: string } }).error.message).toContain('must return a JSON object')
    expect((shapeToolAnswer(counted, toolResult({ content: [{ type: 'text', text: 'x' }] })) as { error: { message: string } }).error.message).toContain(
      'declares an outputSchema but returned no structuredContent',
    )
    // An error result need not hold the contract.
    expect(shapeToolAnswer(counted, toolResult({ content: [{ type: 'text', text: 'CRM down' }], isError: true }))).toEqual({
      result: { content: [{ type: 'text', text: 'CRM down' }], isError: true },
    })
  })

  it('a value with no JSON is the server’s error, not a crash', () => {
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    expect((shapeToolAnswer(tool(), cyclic) as { error: { message: string } }).error.message).toBe('tool "crm_find" returned a value that is not JSON')
  })
})

describe('the load checks, for an outputSchema', () => {
  it('admits an object schema within the bound; leaves out anything else, naming why', () => {
    const r = checkAppMcpTools([
      tool({ name: 'ok', outputSchema: { type: 'object', properties: {} } }),
      tool({ name: 'no_type', outputSchema: { properties: {} } }),
      tool({ name: 'array', outputSchema: { type: 'array' } }),
      tool({ name: 'huge', outputSchema: { type: 'object', description: 'x'.repeat(33 * 1024) } }),
    ])
    expect(r.tools.map((t) => t.name)).toEqual(['ok'])
    expect(r.problems).toEqual([
      '"no_type" has an outputSchema without type "object"; structured results are an object',
      '"array" has an outputSchema of type "array"; structured results are an object',
      expect.stringMatching(/^"huge" has a 33\.\d KiB outputSchema \(at most 32 KiB\)$/),
    ])
  })

  it('the catalog carries the outputSchema, so a schema change is a catalog change', () => {
    const schema = { type: 'object', properties: { n: { type: 'number' } } }
    expect(catalogOf([tool({ outputSchema: schema })])).toEqual([
      { name: 'crm_find', description: 'Find a contact.', inputSchema: { type: 'object', properties: {} }, outputSchema: schema },
    ])
    expect(catalogOf([tool()])[0]).not.toHaveProperty('outputSchema')
  })
})

describe('an unresolved secret handle', () => {
  it('is found in a string, anywhere in it, and anywhere in arrays and objects', () => {
    expect(isUnresolvedHandle('«secret:crm.api_key»')).toBe(true)
    expect(isUnresolvedHandle('Bearer «secret:crm»')).toBe(true)
    expect(isUnresolvedHandle({ headers: [{ authorization: 'Bearer «SECRET:crm.key»' }] })).toBe(true)
    expect(isUnresolvedHandle('sk-live-123')).toBe(false)
    expect(isUnresolvedHandle({ q: 'ada', n: 3, deep: [null, { x: 'y' }] })).toBe(false)
    expect(isUnresolvedHandle(undefined)).toBe(false)
    const cyclic: Record<string, unknown> = { a: 'plain' }
    cyclic.self = cyclic
    expect(isUnresolvedHandle(cyclic)).toBe(false)
  })
})

describe('a call, under its deadline', () => {
  it('the handler is told who calls, for whom, and the app; agent is the caller’s name', async () => {
    let seen: unknown
    const t = tool({ handler: async (_a, ctx) => ((seen = { app: ctx.app, agent: ctx.agent, caller: ctx.caller, acting: ctx.actingUserId }), 'ok') })
    await runAppTools('crm', [t], callOf('crm_find'), { ...facts, actingUserId: 'user-1' })
    expect(seen).toEqual({ app: 'crm', agent: 'crm-agent', caller: { name: 'crm-agent', id: 'def-1', proven: true }, acting: 'user-1' })
  })

  it('the deadline is the server’s timeoutSecs, 120 s when unset', () => {
    expect(DEFAULT_TOOL_TIMEOUT_SECS).toBe(120)
    expect([timeoutSecsOf(null), timeoutSecsOf(undefined), timeoutSecsOf(0), timeoutSecsOf(-5), timeoutSecsOf(30)]).toEqual([120, 120, 120, 120, 30])
  })

  it('a handler that ignores its signal is still answered at the deadline, as an error', async () => {
    const t = tool({ handler: () => new Promise((resolve) => setTimeout(() => resolve('late'), 3_000)) })
    const started = Date.now()
    const r = await runAppTools('crm', [t], callOf('crm_find'), { ...facts, timeoutSecs: 0.05 })
    expect(Date.now() - started).toBeLessThan(1_000)
    expect(r.cut).toBe('deadline')
    expect(r.body).toMatchObject({ result: { isError: true, content: [{ type: 'text', text: expect.stringContaining('did not finish within 0.05 s') }] } })
  })

  it('a handler whose own work rejects on the abort is still a cut, not a tool error', async () => {
    const t = tool({
      handler: (_a, ctx) =>
        new Promise((_, reject) => ctx.signal.addEventListener('abort', () => reject(new Error('fetch aborted')))),
    })
    const r = await runAppTools('crm', [t], callOf('crm_find'), { ...facts, timeoutSecs: 0.05 })
    expect(r.cut).toBe('deadline')
  })

  it('a caller who leaves cuts the call; one already gone never starts the wait', async () => {
    const gone = new AbortController()
    let reason: unknown
    const t = tool({ handler: (_a, ctx) => new Promise((resolve) => ctx.signal.addEventListener('abort', () => resolve((reason = ctx.signal.reason)))) })
    setTimeout(() => gone.abort(new DOMException('left', 'AbortError')), 20)
    const r = await runAppTools('crm', [t], callOf('crm_find'), { ...facts, signal: gone.signal })
    expect(r.cut).toBe('caller')
    expect((reason as DOMException).name).toBe('AbortError')
    const again = await runAppTools('crm', [t], callOf('crm_find'), { ...facts, signal: gone.signal })
    expect(again.cut).toBe('caller')
  })

  it('a call that finishes in time is not cut, and its timer does not outlive it', async () => {
    let signal: AbortSignal | null = null
    const t = tool({ handler: async (_a, ctx) => ((signal = ctx.signal), 'quick') })
    const r = await runAppTools('crm', [t], callOf('crm_find'), { ...facts, timeoutSecs: 0.05 })
    expect(r.cut).toBeNull()
    await new Promise((resolve) => setTimeout(resolve, 100))
    expect(signal!.aborted).toBe(false)
  })

  it('a thrown error is a tool error the model reads; arguments the inputSchema refuses never reach the handler', async () => {
    let ran = false
    const t = tool({
      inputSchema: { type: 'object', properties: { q: { type: 'string' } }, required: ['q'] },
      handler: async (args) => {
        ran = true
        if (args.q === 'boom') throw new Error('CRM is down')
        return 'ok'
      },
    })
    const thrown = await runAppTools('crm', [t], callOf('crm_find', { q: 'boom' }), facts)
    expect(thrown.body).toMatchObject({ result: { isError: true, content: [{ text: 'error: CRM is down' }] } })
    ran = false
    const invalid = await runAppTools('crm', [t], callOf('crm_find', { q: 7 }), facts)
    expect(invalid.body).toMatchObject({ result: { isError: true } })
    expect(ran).toBe(false)
  })
})
