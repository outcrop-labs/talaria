import { describe, expect, it } from 'vitest'
import { catalogLine, emptyHint, serverStatus, type StatusInput } from './mcp-status'

const row = (over: Partial<StatusInput> = {}): StatusInput => ({
  enabled: true,
  builtin: false,
  appSlug: null,
  package: null,
  pkgStatus: null,
  authMode: 'org',
  oauthEnabled: false,
  orgConnected: null,
  oauthMeta: null,
  probe: null,
  headers: {},
  tools: [],
  ...over,
})

describe('serverStatus', () => {
  it('lifecycle first: disabled and package pulls outrank everything', () => {
    expect(serverStatus(row({ enabled: false, probe: { state: 'auth-required', detail: 'x', at: '' } })).kind).toBe('disabled')
    expect(serverStatus(row({ package: {}, pkgStatus: 'pulling' })).kind).toBe('pulling')
    expect(serverStatus(row({ package: {}, pkgStatus: 'error' })).tone).toBe('danger')
  })

  it('an org OAuth server that nobody connected asks for the connect — or the app first', () => {
    const dcr = serverStatus(row({ oauthEnabled: true, orgConnected: false, oauthMeta: { dcr: true, clientSet: false, dcrRejected: null } }))
    expect(dcr.kind).toBe('needs-connect')
    expect(dcr.action).toBe('connect')
    const github = serverStatus(row({ oauthEnabled: true, orgConnected: false, oauthMeta: { dcr: false, clientSet: false, dcrRejected: null } }))
    expect(github.kind).toBe('needs-oauth-app')
    expect(github.action).toBe('setup-oauth-app')
    const vercel = serverStatus(row({ oauthEnabled: true, orgConnected: false, oauthMeta: { dcr: true, clientSet: false, dcrRejected: 'invalid_redirect_uri' } }))
    expect(vercel.kind).toBe('needs-oauth-app')
    // A saved manual client goes back to plain connect.
    const saved = serverStatus(row({ oauthEnabled: true, orgConnected: false, oauthMeta: { dcr: false, clientSet: true, dcrRejected: null } }))
    expect(saved.kind).toBe('needs-connect')
  })

  it('a 401 from discovery on a header server is "needs credentials", with the probe sentence', () => {
    const s = serverStatus(row({ probe: { state: 'auth-required', detail: 'the server requires authentication (401): set its API key', at: '' } }))
    expect(s.kind).toBe('needs-key')
    expect(s.action).toBe('set-credentials')
    expect(s.detail).toContain('401')
  })

  it('per-user servers never ask the org for a key', () => {
    const s = serverStatus(row({ authMode: 'per-user', probe: { state: 'auth-required', detail: 'x', at: '' } }))
    expect(s.kind).toBe('per-user')
    expect(s.action).toBeNull()
  })

  it('reachability failures are danger with the probe detail', () => {
    expect(serverStatus(row({ probe: { state: 'unreachable', detail: 'fetch failed', at: '' } }))).toMatchObject({ kind: 'unreachable', tone: 'danger', detail: 'fetch failed' })
    expect(serverStatus(row({ probe: { state: 'error', detail: 'upstream 503', at: '' } })).kind).toBe('error')
  })

  it('good news: connected OAuth, a successful probe, or a catalog', () => {
    expect(serverStatus(row({ oauthEnabled: true, orgConnected: true })).kind).toBe('connected')
    expect(serverStatus(row({ probe: { state: 'ok', detail: '12 tool(s)', at: '' } })).kind).toBe('ready')
    expect(serverStatus(row({ tools: [{ name: 'a' }] })).kind).toBe('ready')
    expect(serverStatus(row({ builtin: true })).kind).toBe('ready')
  })

  it('a never-probed external server says so instead of claiming anything', () => {
    const s = serverStatus(row())
    expect(s.kind).toBe('untested')
    expect(s.tone).toBe('muted')
  })
})

describe('catalogLine / emptyHint', () => {
  const rel = () => '5 minutes ago'
  it('tells the three phases apart', () => {
    expect(catalogLine({ count: 0, syncedAt: null, syncing: false, phase: 'empty', error: null }, rel)).toBe('Catalog not synced yet')
    expect(catalogLine({ count: 0, syncedAt: null, syncing: true, phase: 'empty', error: null }, rel)).toContain('syncing')
    expect(catalogLine({ count: 1200, syncedAt: null, syncing: true, phase: 'syncing', error: null }, rel)).toBe('Catalog syncing — 1,200 servers so far')
    expect(catalogLine({ count: 21340, syncedAt: 1, syncing: false, phase: 'ready', error: null }, rel)).toBe('21,340 servers · synced 5 minutes ago')
    expect(catalogLine({ count: 21340, syncedAt: 1, syncing: false, phase: 'ready', error: 'registry 503' }, rel)).toContain('last sync failed')
    expect(catalogLine(undefined, rel)).toBe('')
  })

  it('an empty result reads differently before the first sync and after it', () => {
    expect(emptyHint({ count: 0, syncedAt: null, syncing: false, phase: 'empty', error: null }, 'github').title).toBe('Catalog not synced yet')
    expect(emptyHint({ count: 5, syncedAt: null, syncing: true, phase: 'syncing', error: null }, 'github').title).toBe('No match yet')
    expect(emptyHint({ count: 20000, syncedAt: 1, syncing: false, phase: 'ready', error: null }, 'github').title).toBe('No servers match “github”')
    expect(emptyHint(undefined, '').title).toBe('No servers')
  })
})
