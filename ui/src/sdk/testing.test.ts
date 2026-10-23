import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { afterAll, describe, expect, it } from 'vitest'
// Imported the way an app's own test imports them: through the entry ids.
import { callAppTool, memoryStore } from '@talaria/sdk/testing'
import { defineAppMcp, toolResult, type AppMcp } from '@talaria/sdk/server'

// THE QUESTION THIS FILE PINS DOWN: can an app test its tools with no host,
// through the code the gateway runs — and do the docs' examples do what the
// docs say? Every `ts` fence in docs/sdk/mcp.md that defines an mcp.ts is
// copied out VERBATIM, loaded through the SDK ids, and called here; the
// testing fence's two tests are run against the first example, as its
// `import mcp from './mcp'` says. A rename in the contract, or a doc edit
// that stops working, fails here.

const UI = fileURLToPath(new URL('../../', import.meta.url))
const DOC = readFileSync(join(UI, '../docs/sdk/mcp.md'), 'utf8')
const fences = [...DOC.matchAll(/^```ts\n([\s\S]*?)^```$/gm)].map((m) => m[1]!)
const scratch = mkdtempSync(join(UI, '.sdk-doc-examples-mcp-'))
afterAll(() => rmSync(scratch, { recursive: true, force: true }))

/** Each fence that defines an mcp.ts, loaded as its module's default export. */
async function docModules(): Promise<AppMcp[]> {
  const out: AppMcp[] = []
  for (const [i, code] of fences.entries()) {
    if (!code.includes('defineAppMcp(')) continue
    const file = join(scratch, `mcp-${i}.ts`)
    writeFileSync(file, code)
    out.push(((await import(/* @vite-ignore */ pathToFileURL(file).href)) as { default: AppMcp }).default)
  }
  return out
}

describe("docs/sdk/mcp.md's examples", () => {
  it('has the four fences this file runs: the search tool, the structured count, the secret check, and the tests', () => {
    expect(fences.filter((f) => f.includes('defineAppMcp('))).toHaveLength(3)
    const tests = fences.find((f) => f.includes("from '@talaria/sdk/testing'"))
    expect(tests).toBeDefined()
    // The tests below are this fence's, transcribed: hold them to it.
    expect(tests).toContain("await store.insert('contacts', { name: 'Ada Lovelace' })")
    expect(tests).toContain("const r = await callAppTool(mcp, 'contacts_search', { query: 'ada' }, { store })")
    expect(tests).toContain("expect(r.text).toContain('Ada Lovelace')")
    expect(tests).toContain("const r = await callAppTool(mcp, 'contacts_search', { query: 'ada' })")
    expect(tests).toContain("expect(r.text).toBe('[]')")
  })

  it("the testing fence's tests pass against the first example", async () => {
    const [mcp] = await docModules()
    {
      const store = memoryStore()
      await store.insert('contacts', { name: 'Ada Lovelace' })
      const r = await callAppTool(mcp!, 'contacts_search', { query: 'ada' }, { store })
      expect(r.isError).toBe(false)
      expect(r.text).toContain('Ada Lovelace')
    }
    {
      const r = await callAppTool(mcp!, 'contacts_search', { query: 'ada' })
      expect(r.text).toBe('[]')
    }
    // And the search searches: a contact that does not match is not found.
    const store = memoryStore()
    await store.insert('contacts', { name: 'Grace Hopper' })
    expect((await callAppTool(mcp!, 'contacts_search', { query: 'ada' }, { store })).text).toBe('[]')
  })

  it('the structured count answers structuredContent, and an empty store is a tool error without a throw', async () => {
    const [, counted] = await docModules()
    const empty = await callAppTool(counted!, 'contacts_count')
    expect(empty).toMatchObject({ isError: true, text: 'No contacts yet.' })
    expect(empty.structuredContent).toBeUndefined()
    const store = memoryStore()
    await store.insert('contacts', { name: 'Ada' })
    await store.insert('contacts', { name: 'Grace' })
    expect(await callAppTool(counted!, 'contacts_count', {}, { store })).toEqual({
      content: [{ type: 'text', text: '{"count":2}' }],
      structuredContent: { count: 2 },
      isError: false,
      text: '{"count":2}',
      cut: false,
    })
  })

  it('the secret check refuses a handle that did not resolve, naming the fix, before any fetch', async () => {
    const [, , sync] = await docModules()
    const r = await callAppTool(sync!, 'crm_sync', { apiKey: '«secret:crm.api_key»' })
    expect(r.isError).toBe(true)
    expect(r.text).toBe('error: the CRM key did not resolve: its Spendable at list must name talaria-app://crm')
  })
})

describe('callAppTool', () => {
  const echo = defineAppMcp({
    tools: [
      {
        name: 'echo',
        description: 'Echo the call.',
        inputSchema: { type: 'object', properties: { at: { type: 'string' } } },
        handler: async (args, ctx) => ({ args, app: ctx.app, agent: ctx.agent, caller: ctx.caller, actingUserId: ctx.actingUserId }),
      },
      { name: 'Bad Name', description: 'Never listed.', handler: async () => 'x' },
      { name: 'nodesc', description: '', handler: async () => 'x' },
      {
        name: 'slow',
        description: 'Waits for its signal.',
        handler: (_a, ctx) => new Promise((resolve) => ctx.signal.addEventListener('abort', () => resolve('cut'))),
      },
      {
        name: 'listed',
        description: 'A structured list.',
        handler: async () => toolResult({ content: [{ type: 'resource_link', uri: 'https://x.example/1', name: 'one' }], structuredContent: { n: 1 } }),
      },
    ],
  })

  it('defaults to a proven test agent with no acting person, and the arguments cross JSON', async () => {
    const r = await callAppTool(echo, 'echo', { at: new Date('2026-10-22T00:00:00Z') as never, gone: undefined })
    expect(JSON.parse(r.text)).toEqual({
      args: { at: '2026-10-22T00:00:00.000Z' },
      app: 'app',
      agent: 'test-agent',
      caller: { name: 'test-agent', id: 'test-agent', proven: true },
      actingUserId: null,
    })
  })

  it('takes the caller, acting person and app a test sets', async () => {
    const r = await callAppTool(echo, 'echo', {}, { app: 'crm', caller: { name: 'legacy', id: null, proven: false }, actingUserId: 'u-1' })
    expect(JSON.parse(r.text)).toMatchObject({ app: 'crm', agent: 'legacy', caller: { name: 'legacy', id: null, proven: false }, actingUserId: 'u-1' })
  })

  it('a tool the load checks leave out rejects, saying why; so does one outside the granted subset', async () => {
    await expect(callAppTool(echo, 'nodesc')).rejects.toThrow('tool "nodesc" has no description, so the load checks leave it out')
    await expect(callAppTool(echo, 'Bad Name')).rejects.toThrow('has a name with characters other than letters, digits, _ and -')
    await expect(callAppTool(echo, 'echo', {}, { tools: ['slow'] })).rejects.toThrow('tool "echo" is not available here')
    await expect(callAppTool(echo, 'nope')).rejects.toThrow('tool "nope" is not available here')
  })

  it('arguments the inputSchema refuses are a tool error, as the gateway answers them', async () => {
    const r = await callAppTool(echo, 'echo', { at: 7 })
    expect(r.isError).toBe(true)
    expect(r.text).toMatch(/^invalid arguments for echo:/)
  })

  it('cuts at timeoutSecs, and on the signal a test aborts', async () => {
    const late = await callAppTool(echo, 'slow', {}, { timeoutSecs: 0.05 })
    expect(late).toMatchObject({ isError: true, cut: true })
    const gone = new AbortController()
    setTimeout(() => gone.abort(), 10)
    expect((await callAppTool(echo, 'slow', {}, { signal: gone.signal })).cut).toBe(true)
  })

  it('passes a toolResult through as the agent gets it', async () => {
    const r = await callAppTool(echo, 'listed')
    expect(r).toEqual({
      content: [{ type: 'resource_link', uri: 'https://x.example/1', name: 'one' }],
      structuredContent: { n: 1 },
      isError: false,
      text: '',
      cut: false,
    })
  })
})

describe('memoryStore', () => {
  it('answers like the installed store: JSON copies, UUID ids, a one-level merge, newest first', async () => {
    const store = memoryStore()
    const a = await store.insert('c', { name: 'Ada', tags: ['x'], at: new Date('2026-01-01T00:00:00Z') as never })
    expect(a.id).toMatch(/^[0-9a-f-]{36}$/)
    expect(a.data).toEqual({ name: 'Ada', tags: ['x'], at: '2026-01-01T00:00:00.000Z' })
    // A returned doc is a copy: changing it changes nothing stored.
    ;(a.data.tags as string[]).push('y')
    expect((await store.get('c', a.id))!.data.tags).toEqual(['x'])
    const b = await store.insert('c', { name: 'Bob' })
    expect((await store.list('c')).map((d) => d.data.name)).toEqual(['Bob', 'Ada'])
    expect((await store.list('c', { newestFirst: false })).map((d) => d.data.name)).toEqual(['Ada', 'Bob'])
    expect((await store.list('c', { limit: 1, offset: 1 })).map((d) => d.data.name)).toEqual(['Ada'])
    expect((await store.list('c', { limit: 0 })).length).toBe(1) // clamped to at least one, as Postgres pages
    // jsonb `||`: top-level keys replace, nothing deeper merges.
    const merged = await store.update('c', b.id, { tags: ['z'], role: 'eng' })
    expect(merged!.data).toEqual({ name: 'Bob', tags: ['z'], role: 'eng' })
    expect(await store.get('c', b.id.toUpperCase())).not.toBeNull()
    expect(await store.count('c')).toBe(2)
    expect(await store.count('other')).toBe(0)
  })

  it('an id that is not a UUID names no document', async () => {
    const store = memoryStore()
    expect(await store.get('c', '123')).toBeNull()
    expect(await store.update('c', '123', { a: 1 })).toBeNull()
    expect(await store.remove('c', '123')).toBe(false)
    expect(await store.get('c', '00000000-0000-4000-8000-000000000000')).toBeNull()
  })

  it('remove and wipe', async () => {
    const store = memoryStore()
    const a = await store.insert('c', { n: 1 })
    await store.insert('d', { n: 2 })
    expect(await store.remove('c', a.id)).toBe(true)
    expect(await store.remove('c', a.id)).toBe(false)
    await store.wipe()
    expect(await store.count('d')).toBe(0)
  })
})
