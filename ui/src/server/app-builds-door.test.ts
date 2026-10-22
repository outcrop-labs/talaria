// The /app-builds/ door: the client files only, and only past the app gate.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest'

let gate: { ok: true } | { ok: false; response: Response } = { ok: true }
const authorizeApp = vi.fn(async () => gate)
vi.mock('./app-access', () => ({ authorizeApp }))

const builds = mkdtempSync(join(tmpdir(), 'app-builds-door-'))
process.env.TALARIA_APP_BUILDS_DIR = builds
const KEY = '0123456789abcdef'
const dir = join(builds, 'crm', KEY)
mkdirSync(join(dir, 'assets', 'img'), { recursive: true })
mkdirSync(join(dir, '.cache-client'), { recursive: true })
for (const [rel, body] of [
  ['app.js', 'export default {}'],
  ['app.css', '.a{}'],
  ['assets/chunk-AbC_1.js', 'export const x = 1'],
  ['assets/img/logo.svg', '<svg/>'],
  ['server.js', 'SECRET SERVER CODE'],
  ['mcp.js', 'SECRET MCP CODE'],
  ['.cache-client/x.json', '{}'],
] as const) writeFileSync(join(dir, rel), body)
writeFileSync(join(builds, 'crm', 'current.json'), JSON.stringify({ key: KEY }))

const { appBuildAsset, serveAppBuild } = await import('./app-builds-door')

afterAll(() => rmSync(builds, { recursive: true, force: true }))
beforeEach(() => {
  gate = { ok: true }
  authorizeApp.mockClear()
})

const get = (path: string, method = 'GET') => serveAppBuild(new Request(`http://app.test${path}`, { method }))

describe('appBuildAsset', () => {
  it('names app.js, app.css and files under assets/', () => {
    expect(appBuildAsset(`/app-builds/crm/${KEY}/app.js`)).toEqual({ slug: 'crm', key: KEY, rel: ['app.js'] })
    expect(appBuildAsset(`/app-builds/crm/${KEY}/app.css`)).toEqual({ slug: 'crm', key: KEY, rel: ['app.css'] })
    expect(appBuildAsset(`/app-builds/crm/${KEY}/assets/img/logo.svg`)).toEqual({ slug: 'crm', key: KEY, rel: ['assets', 'img', 'logo.svg'] })
  })

  it('refuses the server half, the key file, dotfiles and traversal', () => {
    for (const p of [
      `/app-builds/crm/${KEY}/server.js`,
      `/app-builds/crm/${KEY}/mcp.js`,
      `/app-builds/crm/current.json`,
      `/app-builds/crm/${KEY}/.cache-client/x.json`,
      `/app-builds/crm/${KEY}/assets/.hidden`,
      `/app-builds/crm/${KEY}/assets/`,
      `/app-builds/crm/${KEY}/assets/..%2f..%2fserver.js`,
      `/app-builds/crm/${KEY}/assets/%2e%2e/server.js`,
      `/app-builds/crm/${KEY}/app.js/x`,
      `/app-builds/crm/NOTHEX/app.js`,
      `/app-builds/Crm/${KEY}/app.js`,
      `/app-builds/crm/${KEY}/assets/%E0%A4%A`,
      '/app-builds/',
      '/somewhere/else',
    ]) {
      expect(appBuildAsset(p), p).toBeNull()
    }
  })
})

describe('serveAppBuild', () => {
  it('serves the client to a granted session, private to that browser', async () => {
    const res = await get(`/app-builds/crm/${KEY}/app.js`)
    expect(res.status).toBe(200)
    expect(await res.text()).toBe('export default {}')
    expect(res.headers.get('content-type')).toBe('application/javascript')
    expect(res.headers.get('cache-control')).toMatch(/^private/)
    expect(authorizeApp).toHaveBeenCalledWith(expect.any(Request), 'crm')
    expect((await get(`/app-builds/crm/${KEY}/assets/chunk-AbC_1.js`)).status).toBe(200)
    expect(await (await get(`/app-builds/crm/${KEY}/app.css`)).text()).toBe('.a{}')
  })

  it('never serves server.js, mcp.js or current.json — not even to an admin', async () => {
    for (const p of [`/app-builds/crm/${KEY}/server.js`, `/app-builds/crm/${KEY}/mcp.js`, '/app-builds/crm/current.json']) {
      const res = await get(p)
      expect(res.status, p).toBe(404)
      expect(await res.text()).not.toMatch(/SECRET|key/)
    }
    // Refused on the path alone: the gate is never consulted.
    expect(authorizeApp).not.toHaveBeenCalled()
  })

  it('answers the gate\'s refusal and reads nothing', async () => {
    gate = { ok: false, response: new Response('{"error":"forbidden"}', { status: 403 }) }
    const res = await get(`/app-builds/crm/${KEY}/app.js`)
    expect(res.status).toBe(403)
    expect(await res.text()).not.toContain('export default')
  })

  it('404s a client file that is not there, and refuses writes', async () => {
    expect((await get(`/app-builds/crm/${KEY}/assets/missing.js`)).status).toBe(404)
    expect((await get(`/app-builds/crm/ffff/app.js`)).status).toBe(404)
    expect((await get(`/app-builds/crm/${KEY}/app.js`, 'POST')).status).toBe(405)
  })

  it('answers HEAD with headers and no body', async () => {
    const res = await get(`/app-builds/crm/${KEY}/app.js`, 'HEAD')
    expect(res.status).toBe(200)
    expect(res.headers.get('content-length')).toBe(String('export default {}'.length))
    expect(await res.text()).toBe('')
  })
})
