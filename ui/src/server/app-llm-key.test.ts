import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest'

// THE KEY AN APP'S CALLS ARE SIGNED WITH, as this host holds it: asked of the
// api once, over the internal route, and kept; asked again only after the
// relay refused the copy held here.
vi.mock('./secretbox', () => ({ internalToken: () => 'internal-token' }))
vi.mock('./rust-proxy', () => ({ rustApiUrl: () => 'http://api.test' }))

const asks: Array<{ url: string; headers: Record<string, string> }> = []
let answer: () => Response = () => new Response(JSON.stringify({ key: 'tlk_1' }), { status: 200 })
const realFetch = globalThis.fetch
globalThis.fetch = (async (url: string, init: { headers: Record<string, string> }) => {
  asks.push({ url, headers: init.headers })
  return answer()
}) as unknown as typeof fetch
afterAll(() => {
  globalThis.fetch = realFetch
})

const { appGatewayKey, forgetAppGatewayKey } = await import('./app-llm-key')

beforeEach(() => {
  asks.length = 0
  forgetAppGatewayKey('crm')
  answer = () => new Response(JSON.stringify({ key: 'tlk_1' }), { status: 200 })
})

describe('appGatewayKey', () => {
  it('asks the api for the app’s key with the internal credential, once', async () => {
    const [a, b] = await Promise.all([appGatewayKey('crm'), appGatewayKey('crm')])
    expect([a, b]).toEqual(['tlk_1', 'tlk_1'])
    expect(await appGatewayKey('crm')).toBe('tlk_1')
    expect(asks).toHaveLength(1)
    expect(asks[0]!.url).toBe('http://api.test/api/internal/apps/crm/llm-key')
    expect(asks[0]!.headers['x-talaria-internal']).toBe('internal-token')
  })

  it('asks again after the relay refused the held copy', async () => {
    await appGatewayKey('crm')
    answer = () => new Response(JSON.stringify({ key: 'tlk_2' }), { status: 200 })
    forgetAppGatewayKey('crm')
    expect(await appGatewayKey('crm')).toBe('tlk_2')
    expect(asks).toHaveLength(2)
  })

  it('says why there is no key, and keeps no failure', async () => {
    answer = () => new Response(JSON.stringify({ error: 'app "crm" is not enabled, so it has no gateway key' }), { status: 404 })
    await expect(appGatewayKey('crm')).rejects.toThrow(/not enabled/)
    answer = () => new Response(JSON.stringify({ key: 'tlk_3' }), { status: 200 })
    expect(await appGatewayKey('crm')).toBe('tlk_3')
  })
})
