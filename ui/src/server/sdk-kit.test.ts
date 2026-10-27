// The kit route's serving half: which kit on disk counts, and the headers a
// sandbox installs by. (Who may fetch it is the route's: an agent's own
// credential or a session — routes/api/sdk.kit.tgz.ts.)
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { readServedKit, serveKit, type ServedKit } from './sdk-kit'

let dir = ''
const kit: ServedKit = { version: 'dev-1-abcd1234', hostBuild: 'dev-1', sdk: '1.0.0', digest: 'f00d', bytes: 5, sources: 's' }

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), 'sdk-kit-served-'))
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'kit.tgz'), 'bytes')
  writeFileSync(join(dir, 'kit.json'), JSON.stringify(kit))
})
afterAll(() => rmSync(dir, { recursive: true, force: true }))

describe('readServedKit', () => {
  it('reads the kit the builder wrote, and nothing half-written', () => {
    expect(readServedKit(dir)).toEqual(kit)
    writeFileSync(join(dir, 'kit.json'), JSON.stringify({ ...kit, bytes: 6 }))
    expect(readServedKit(dir)).toBeNull()
    writeFileSync(join(dir, 'kit.json'), JSON.stringify(kit))
    expect(readServedKit(join(dir, 'nowhere'))).toBeNull()
  })
})

describe('serveKit', () => {
  const current = async () => kit
  it('serves the archive with its version, SDK version and etag', async () => {
    const res = await serveKit(new Request('http://x/api/sdk/kit.tgz'), false, { current, dir })
    expect(res.status).toBe(200)
    expect(res.headers.get('x-talaria-kit-version')).toBe('dev-1-abcd1234')
    expect(res.headers.get('x-talaria-sdk-version')).toBe('1.0.0')
    expect(res.headers.get('etag')).toBe('"f00d"')
    expect(res.headers.get('content-type')).toBe('application/gzip')
    expect(res.headers.get('content-disposition')).toBe('attachment; filename="talaria-sdk-kit-dev-1-abcd1234.tgz"')
    expect(await res.text()).toBe('bytes')
  })

  it('answers HEAD with the headers alone, and a matching etag with 304', async () => {
    const head = await serveKit(new Request('http://x/api/sdk/kit.tgz', { method: 'HEAD' }), true, { current, dir })
    expect(head.status).toBe(200)
    expect(head.headers.get('content-length')).toBe('5')
    expect(await head.text()).toBe('')
    const same = await serveKit(new Request('http://x/api/sdk/kit.tgz', { headers: { 'if-none-match': '"other", "f00d"' } }), false, { current, dir })
    expect(same.status).toBe(304)
    expect(same.headers.get('x-talaria-kit-version')).toBe('dev-1-abcd1234')
  })

  it('says why there is no kit', async () => {
    const res = await serveKit(new Request('http://x/api/sdk/kit.tgz'), false, { current: async () => ({ error: 'this build ships no SDK kit' }), dir })
    expect(res.status).toBe(503)
    expect(await res.json()).toEqual({ error: 'this build ships no SDK kit' })
  })
})
