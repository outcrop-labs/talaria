import { mkdtempSync, readFileSync, rmSync, existsSync, writeFileSync, mkdirSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest'

// THE QUESTION THIS FILE PINS DOWN: what an agent can see of an app's mcp.ts.
// A tool that fails its checks is left out and named — never listed, never
// callable, never catalogued — and never takes the app or its other tools down
// with it. The catalog the registry row holds is written to the app's data
// directory and handed to the api, and an app that is not enabled serves
// nothing at all.

const state = vi.hoisted(() => ({
  enabled: [] as string[],
  resyncs: [] as string[],
  resyncAnswer: ['app-crm'] as string[] | null,
  module: null as unknown,
}))

const buildsDir = mkdtempSync(join(tmpdir(), 'app-mcp-test-builds-'))
const dataDir = mkdtempSync(join(tmpdir(), 'app-mcp-test-data-'))
process.env.TALARIA_APP_BUILDS_DIR = buildsDir
process.env.TALARIA_APP_DATA_DIR = dataDir

vi.mock('./audit', () => ({
  getSetting: async (key: string, fallback: unknown) => (key === 'apps_enabled' ? state.enabled : fallback),
}))
vi.mock('./internal-api', () => ({
  resyncAppMcpServers: async (reason: string) => {
    state.resyncs.push(reason)
    return state.resyncAnswer
  },
}))
vi.mock('./app-load', () => ({
  appHasMcp: () => true,
  loadAppMcp: async () => state.module,
}))
vi.mock('./app-store', () => ({ storeFor: () => ({}) }))

const mcp = await import('./app-mcp')

const handler = async () => 'ok'
const tool = (over: Record<string, unknown> = {}) => ({
  name: 'crm_find',
  description: 'Find a contact by name.',
  inputSchema: { type: 'object', properties: { q: { type: 'string' } } },
  handler,
  ...over,
})

afterAll(() => {
  rmSync(buildsDir, { recursive: true, force: true })
  rmSync(dataDir, { recursive: true, force: true })
})

describe('checkAppMcpTools', () => {
  it('keeps every tool that passes, in order', () => {
    const r = mcp.checkAppMcpTools([tool(), tool({ name: 'crm_add', inputSchema: undefined })])
    expect(r.tools.map((t) => t.name)).toEqual(['crm_find', 'crm_add'])
    expect(r.problems).toEqual([])
  })

  it('leaves out — and names — every tool that fails, keeping the rest', () => {
    const big = { type: 'object', properties: { x: { type: 'string', description: 'y'.repeat(33 * 1024) } } }
    const r = mcp.checkAppMcpTools([
      tool(),
      tool({ name: '' }),
      tool({ name: 'n'.repeat(201) }),
      tool({ name: 'crm find' }),
      tool({ name: 'crm.find' }),
      tool(), // a second crm_find
      tool({ name: 'crm_blank', description: '   ' }),
      tool({ name: 'crm_nohandler', handler: undefined }),
      tool({ name: 'crm_array', inputSchema: [] }),
      tool({ name: 'crm_string', inputSchema: { type: 'string' } }),
      tool({ name: 'crm_huge', inputSchema: big }),
      null,
    ])
    expect(r.tools.map((t) => t.name)).toEqual(['crm_find'])
    expect(r.problems).toEqual([
      'tool #2 has no name',
      `"${'n'.repeat(60)}…" has a 201-character name (at most 200)`,
      '"crm find" has a name with characters other than letters, digits, _ and -',
      '"crm.find" has a name with characters other than letters, digits, _ and -',
      '"crm_find" is defined twice; only the first definition is kept',
      '"crm_blank" has no description',
      '"crm_nohandler" has no handler function',
      '"crm_array" has an inputSchema that is not a JSON Schema object',
      '"crm_string" has an inputSchema of type "string"; tool arguments are an object',
      '"crm_huge" has a 33.1 KiB inputSchema (at most 32 KiB)',
      'tool #12 is not a tool definition',
    ])
  })

  it('a name of exactly 200 characters and a schema of exactly 32 KiB pass', () => {
    const name = 'a'.repeat(200)
    const pad = 32 * 1024 - JSON.stringify({ type: 'object', description: '' }).length
    const schema = { type: 'object', description: 'd'.repeat(pad) }
    expect(Buffer.byteLength(JSON.stringify(schema))).toBe(32 * 1024)
    expect(mcp.checkAppMcpTools([tool({ name, inputSchema: schema })]).problems).toEqual([])
  })

  it('a later valid definition counts when the first one failed', () => {
    const r = mcp.checkAppMcpTools([tool({ description: '' }), tool()])
    expect(r.tools).toHaveLength(1)
    expect(r.problems).toEqual(['"crm_find" has no description'])
  })
})

describe('catalogOf / catalogProbe', () => {
  it('the catalog is what tools/list serves, descriptions clipped to the registry bound', () => {
    const t = tool({ description: 'd'.repeat(400), inputSchema: undefined }) as never
    expect(mcp.catalogOf([t])).toEqual([{ name: 'crm_find', description: 'd'.repeat(300), inputSchema: { type: 'object', properties: {} } }])
  })

  it('reads ok with the count, or error naming what was left out — within the api’s 300 characters', () => {
    const at = new Date('2026-10-22T12:00:00.000Z')
    expect(mcp.catalogProbe({ tools: [tool() as never], problems: [] }, at)).toEqual({ state: 'ok', detail: '1 tool', at: '2026-10-22T12:00:00.000Z' })
    const left = mcp.catalogProbe({ tools: [], problems: ['"x y" has a name with characters other than letters, digits, _ and -'] }, at)
    expect(left.state).toBe('error')
    expect(left.detail).toBe('0 tools; 1 left out: "x y" has a name with characters other than letters, digits, _ and -')
    const long = mcp.catalogProbe({ tools: [], problems: Array.from({ length: 20 }, (_, i) => `"t${i}" has no description`) }, at)
    expect(long.detail.length).toBe(300)
    expect(long.detail.endsWith('…')).toBe(true)
  })
})

describe('syncAppMcpCatalog', () => {
  beforeEach(() => {
    state.enabled = []
    state.resyncs = []
    state.resyncAnswer = ['app-crm']
  })

  const read = (slug: string) => JSON.parse(readFileSync(mcp.appMcpCatalogPath(slug), 'utf8'))

  it('writes the checked catalog to the app’s data directory, in the shape the api copies into the row', async () => {
    await mcp.syncAppMcpCatalog('crm', mcp.checkAppMcpTools([tool(), tool({ name: 'Bad Name' })]))
    // Never into app-builds/, which the host serves to anyone.
    expect(mcp.appMcpCatalogPath('crm')).toBe(join(dataDir, 'crm', 'mcp-catalog.json'))
    expect(existsSync(join(buildsDir, 'crm', 'mcp-catalog.json'))).toBe(false)
    // The directory the app database's db.env lands in later: owner-only.
    expect(statSync(join(dataDir, 'crm')).mode & 0o777).toBe(0o700)
    const file = read('crm')
    expect(file.tools.map((t: { name: string }) => t.name)).toEqual(['crm_find'])
    expect(file.probe.state).toBe('error')
    expect(file.probe.detail).toContain('"Bad Name" has a name')
    // Not enabled: no row to fill, so the api is not asked.
    expect(state.resyncs).toEqual([])
  })

  it('asks the api to copy it for an enabled app — once per distinct catalog', async () => {
    state.enabled = ['crm2']
    const checked = mcp.checkAppMcpTools([tool()])
    await mcp.syncAppMcpCatalog('crm2', checked)
    await mcp.syncAppMcpCatalog('crm2', mcp.checkAppMcpTools([tool()]))
    expect(state.resyncs).toEqual(['crm2: catalog'])
    await mcp.syncAppMcpCatalog('crm2', mcp.checkAppMcpTools([tool(), tool({ name: 'crm_add' })]))
    expect(state.resyncs).toEqual(['crm2: catalog', 'crm2: catalog'])
    expect(read('crm2').probe).toMatchObject({ state: 'ok', detail: '2 tools' })
  })

  it('asks again next time when the api did not take it', async () => {
    state.enabled = ['crm3']
    state.resyncAnswer = null
    await mcp.syncAppMcpCatalog('crm3', mcp.checkAppMcpTools([tool()]))
    state.resyncAnswer = ['app-crm3']
    await mcp.syncAppMcpCatalog('crm3', mcp.checkAppMcpTools([tool()]))
    expect(state.resyncs).toEqual(['crm3: catalog', 'crm3: catalog'])
  })

  it('writes a remembered catalog again when its file is gone (an uninstall removed it)', async () => {
    await mcp.syncAppMcpCatalog('crm5', mcp.checkAppMcpTools([tool()]))
    rmSync(mcp.appMcpCatalogPath('crm5'))
    await mcp.syncAppMcpCatalog('crm5', mcp.checkAppMcpTools([tool()]))
    expect(read('crm5').tools).toHaveLength(1)
  })

  it('an app that stops publishing MCP drops its catalog, and the api is asked to turn its row off', async () => {
    state.enabled = ['crm4']
    mkdirSync(join(dataDir, 'crm4'), { recursive: true })
    writeFileSync(mcp.appMcpCatalogPath('crm4'), '{}')
    await mcp.clearAppMcpCatalog('crm4')
    expect(existsSync(mcp.appMcpCatalogPath('crm4'))).toBe(false)
    expect(state.resyncs).toEqual(['crm4: no MCP surface'])
    // Nothing to drop: nothing asked.
    await mcp.clearAppMcpCatalog('crm4')
    expect(state.resyncs).toHaveLength(1)
  })
})

describe('dispatchAppMcp', () => {
  const rpc = (method: string, params?: Record<string, unknown>) => ({ jsonrpc: '2.0', id: 1, method, params })

  it('lists and calls only the tools that passed', async () => {
    state.module = { default: { tools: [tool(), tool({ name: 'crm_broken', description: '' })] } }
    const list = await mcp.dispatchAppMcp('crm', rpc('tools/list'), 'agent-a', null)
    expect((list.body as { result: { tools: Array<{ name: string }> } }).result.tools.map((t) => t.name)).toEqual(['crm_find'])
    const call = await mcp.dispatchAppMcp('crm', rpc('tools/call', { name: 'crm_broken', arguments: {} }), 'agent-a', null)
    expect((call.body as { error: { message: string } }).error.message).toBe('tool "crm_broken" is not available here')
    const ok = await mcp.dispatchAppMcp('crm', rpc('tools/call', { name: 'crm_find', arguments: { q: 'ada' } }), 'agent-a', null)
    expect((ok.body as { result: { content: Array<{ text: string }> } }).result.content[0]!.text).toBe('ok')
  })
})

describe('appMcpOff', () => {
  it('refuses an app that is not enabled, and nothing else', async () => {
    state.enabled = ['crm']
    expect(await mcp.appMcpOff('crm')).toBeNull()
    expect(await mcp.appMcpOff('other')).toBe('app "other" is not enabled')
    expect(await mcp.appMcpOff(null)).toBeNull()
  })

  it('refuses a row the api turned off, with its reason; a disabled app says so first', async () => {
    state.enabled = ['crm']
    const why = 'the app no longer publishes MCP tools'
    expect(await mcp.appMcpOff('crm', why)).toBe(`app "crm" is not serving: ${why}`)
    expect(await mcp.appMcpOff('crm', null)).toBeNull()
    expect(await mcp.appMcpOff('other', why)).toBe('app "other" is not enabled')
    expect(await mcp.appMcpOff(null, why)).toBeNull()
  })

  it('refuses every app while the enabled set is not a list of slugs — never a slug found inside a string', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    for (const malformed of ['crm-suite', { crm: true }, ['crm', 7]]) {
      state.enabled = malformed as unknown as string[]
      expect(await mcp.appMcpOff('crm')).toBe('could not check that app "crm" is enabled — it serves nothing until that reads')
    }
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('the apps_enabled setting is not a list of app slugs'))
    warn.mockRestore()
    state.enabled = ['crm']
    expect(await mcp.appMcpOff('crm')).toBeNull()
  })
})
