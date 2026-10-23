import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest'

// failHealth's collaborators: the settings it writes, the audit it leaves and
// the api it asks. The build status is real, in a scratch builds directory.
const st = vi.hoisted(() => ({
  settings: new Map<string, unknown>(),
  audits: [] as Array<Record<string, unknown>>,
  resyncs: [] as string[],
  resyncAnswer: ['app-notes'] as string[] | null,
}))
vi.mock('./audit', () => ({
  getSetting: async (key: string, fallback: unknown) => (st.settings.has(key) ? st.settings.get(key) : fallback),
  setSetting: async (key: string, value: unknown) => {
    st.settings.set(key, value)
  },
  logAudit: async (entry: Record<string, unknown>) => {
    st.audits.push(entry)
  },
}))
vi.mock('./internal-api', () => ({
  resyncAppMcpServers: async (reason: string) => {
    st.resyncs.push(reason)
    return st.resyncAnswer
  },
}))

const buildsDir = mkdtempSync(join(tmpdir(), 'app-health-test-'))
process.env.TALARIA_APP_BUILDS_DIR = buildsDir
afterAll(() => rmSync(buildsDir, { recursive: true, force: true }))

const { clientArtifactOk, failHealth, moduleHasFetch, moduleHasMcpTools, moduleHasSurfaces } = await import('./app-health')
const { readCurrent } = await import('./app-build/status')

describe('moduleHasFetch', () => {
  it('accepts defineAppServer shape', () => {
    expect(moduleHasFetch({ default: { fetch: async () => new Response() } })).toBe(true)
  })
  it('rejects a missing or non-function fetch', () => {
    expect(moduleHasFetch(null)).toBe(false)
    expect(moduleHasFetch({ default: {} })).toBe(false)
    expect(moduleHasFetch({ default: { fetch: 1 } })).toBe(false)
  })
})

describe('moduleHasMcpTools', () => {
  it('accepts defineAppMcp shape', () => {
    expect(moduleHasMcpTools({ default: { tools: [] } })).toBe(true)
  })
  it('rejects a module with no tools array', () => {
    expect(moduleHasMcpTools({ default: {} })).toBe(false)
    expect(moduleHasMcpTools({ tools: [] })).toBe(false)
  })
})

describe('moduleHasSurfaces', () => {
  it('accepts defineApp with at least one surface', () => {
    expect(moduleHasSurfaces({ default: { work: {} } })).toBe(true)
    expect(moduleHasSurfaces({ default: { manage: {}, settings: {} } })).toBe(true)
  })
  it('rejects an empty or missing default', () => {
    expect(moduleHasSurfaces(null)).toBe(false)
    expect(moduleHasSurfaces({ default: {} })).toBe(false)
    expect(moduleHasSurfaces({ work: {} })).toBe(false)
  })
})

describe('clientArtifactOk', () => {
  it('accepts an ESM bundle with an export', () => {
    expect(clientArtifactOk('const x = 1;\nexport { x as default };\n')).toBe(true)
  })
  it("accepts Rollup's minified default export and a plain one", () => {
    expect(clientArtifactOk('import*as r from"/runtime/rt-svelte-internal-client.js";const t={work:1};export{t as default};')).toBe(true)
    expect(clientArtifactOk('const surfaces = { work: 1 };\nexport default surfaces;\n')).toBe(true)
  })
  it('rejects empty or non-module output', () => {
    expect(clientArtifactOk('')).toBe(false)
    expect(clientArtifactOk('(function(){var app=1})();')).toBe(false)
  })
  it('rejects a bundle whose entry lost its default export', () => {
    // What Vite's default preserveEntrySignatures (false) produced: the
    // entry's code, its imports, and no export at all.
    expect(clientArtifactOk('import*as r from"/runtime/rt-svelte.js";const t={work:function(){}};r.from_html("");')).toBe(false)
    expect(clientArtifactOk('const work = 1;\nconst manage = 2;\nexport { work, manage };\n')).toBe(false)
  })
})

describe('failHealth', () => {
  beforeEach(() => {
    st.settings = new Map()
    st.audits = []
    st.resyncs = []
    st.resyncAnswer = ['app-notes']
  })

  // The api's resync is what takes the app's MCP server off its agents: it
  // turns the row off and rolls the carriers, and keeps every grant
  // (talaria-apps `sync_app_mcp_servers`; its live test proves the rows).
  // failHealth itself writes no registry row at all — it used to be the
  // trigger of a delete that cascaded them.
  it('marks the build failed, turns the app off, and asks the api to take its MCP server off agents', async () => {
    st.settings.set('apps_enabled', ['crm', 'notes'])
    await failHealth('crm', 'mcp.ts: SyntaxError')
    expect(readCurrent('crm')).toMatchObject({ status: 'failed', error: 'mcp.ts: SyntaxError' })
    expect(st.settings.get('apps_enabled')).toEqual(['notes'])
    expect(st.audits).toEqual([
      expect.objectContaining({ actor: 'system:app-health', action: 'app.disable', targetId: 'crm', targetLabel: 'mcp.ts: SyntaxError' }),
    ])
    expect(st.resyncs).toEqual(['crm: turned off'])
  })

  it('asks again for an app already off, so a row an earlier failure left serving is turned off too', async () => {
    st.settings.set('apps_enabled', ['notes'])
    await failHealth('crm', 'database did not start')
    expect(st.settings.get('apps_enabled')).toEqual(['notes'])
    expect(st.audits).toEqual([])
    expect(st.resyncs).toEqual(['crm: failed'])
  })

  it('still turns the app off when the api cannot be asked', async () => {
    st.settings.set('apps_enabled', ['crm'])
    st.resyncAnswer = null
    await expect(failHealth('crm', 'boom')).resolves.toBeUndefined()
    expect(st.settings.get('apps_enabled')).toEqual([])
    expect(st.resyncs).toEqual(['crm: turned off'])
  })
})
