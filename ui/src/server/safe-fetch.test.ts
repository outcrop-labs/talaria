import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// DNS is the one thing here that would otherwise be non-deterministic and
// network-dependent, so it is stubbed — and only it. Address classification,
// the allowlist parser, redirect re-validation and the body cap are all real.
const dnsLookup = vi.fn<(host: string, opts: unknown) => Promise<Array<{ address: string; family: number }>>>()
vi.mock('node:dns/promises', () => ({ lookup: dnsLookup }))
vi.mock('node:dns', () => ({ lookup: vi.fn() }))

const { assertFetchableUrl, blockedAddressReason, BlockedUrlError, hostAllowed, safeFetch } = await import(
  '@/server/safe-fetch'
)

const PUBLIC_V4 = [{ address: '93.184.216.34', family: 4 }]
const resolvesTo = (...addresses: string[]) =>
  dnsLookup.mockResolvedValue(addresses.map((address) => ({ address, family: address.includes(':') ? 6 : 4 })))

beforeEach(() => {
  delete process.env.TALARIA_FETCH_ALLOW_HOSTS
  dnsLookup.mockReset()
  dnsLookup.mockResolvedValue(PUBLIC_V4)
})
afterEach(() => {
  vi.unstubAllGlobals()
})

// ── Address classification ──────────────────────────────────────────────────

describe('blockedAddressReason — IPv4', () => {
  it.each([
    ['127.0.0.1', 'loopback'],
    ['127.255.255.254', 'loopback'],
    ['10.0.0.1', 'private network (RFC1918)'],
    ['10.255.255.255', 'private network (RFC1918)'],
    ['172.16.0.1', 'private network (RFC1918)'],
    ['172.31.255.255', 'private network (RFC1918)'],
    ['192.168.1.1', 'private network (RFC1918)'],
    ['169.254.169.254', 'link-local — cloud instance metadata lives here'],
    ['100.64.0.1', 'carrier-grade NAT (RFC6598)'],
    ['100.127.255.255', 'carrier-grade NAT (RFC6598)'],
    ['0.0.0.0', 'unspecified/this-network'],
    ['192.0.0.1', 'IETF protocol assignments'],
    ['192.0.2.7', 'documentation range'],
    ['198.18.0.1', 'benchmarking range'],
    ['198.51.100.7', 'documentation range'],
    ['203.0.113.7', 'documentation range'],
    ['224.0.0.1', 'multicast'],
    ['255.255.255.255', 'reserved'],
  ])('refuses %s', (ip, why) => {
    expect(blockedAddressReason(ip)).toBe(why)
  })

  it.each([
    '8.8.8.8',
    '1.1.1.1',
    '93.184.216.34',
    '172.15.255.255', // just below the RFC1918 /12
    '172.32.0.1', // just above it
    '100.128.0.1', // just above the CGNAT /10
    '11.0.0.1',
  ])('allows the public address %s', (ip) => {
    expect(blockedAddressReason(ip)).toBeNull()
  })
})

describe('blockedAddressReason — IPv6', () => {
  it.each([
    ['::1', 'IPv6 loopback'],
    ['::', 'unspecified'],
    ['fc00::1', 'IPv6 unique-local (ULA)'],
    ['fd12:3456:789a::1', 'IPv6 unique-local (ULA)'],
    ['fe80::1', 'IPv6 link-local'],
    ['fe80::a00:27ff:fe4e:66a1', 'IPv6 link-local'],
    ['ff02::1', 'IPv6 multicast'],
    ['2001:db8::1', 'documentation range'],
  ])('refuses %s', (ip, why) => {
    expect(blockedAddressReason(ip)).toBe(why)
  })

  it.each(['2606:4700:4700::1111', '2a00:1450:4001:81b::200e'])('allows the public address %s', (ip) => {
    expect(blockedAddressReason(ip)).toBeNull()
  })
})

describe('blockedAddressReason — IPv4 wearing an IPv6 costume', () => {
  it.each([
    ['::ffff:127.0.0.1', 'loopback'],
    ['::ffff:7f00:1', 'loopback'], // the same address, hex form
    ['::ffff:169.254.169.254', 'link-local — cloud instance metadata lives here'],
    ['::ffff:10.0.0.1', 'private network (RFC1918)'],
    ['::ffff:192.168.0.1', 'private network (RFC1918)'],
    ['2002:a00:1::', 'private network (RFC1918)'], // 6to4 wrapper around 10.0.0.1
    ['2002:7f00:1::', 'loopback'], // 6to4 wrapper around 127.0.0.1
    ['64:ff9b::10.0.0.1', 'private network (RFC1918)'], // NAT64
    ['64:ff9b::a9fe:a9fe', 'link-local — cloud instance metadata lives here'],
  ])('unwraps and refuses %s', (ip, why) => {
    expect(blockedAddressReason(ip)).toBe(why)
  })

  it('still allows a genuinely public address behind the same wrappers', () => {
    expect(blockedAddressReason('::ffff:8.8.8.8')).toBeNull()
    expect(blockedAddressReason('2002:808:808::')).toBeNull()
  })

  it('does not confuse a normal v6 address for a wrapped v4 one', () => {
    expect(blockedAddressReason('2001:4860:4860::8888')).toBeNull()
  })
})

describe('blockedAddressReason — junk', () => {
  it.each(['', 'not-an-ip', '999.1.1.1', '1.2.3', '1.2.3.4.5', '10.0.0.-1', 'gggg::1', '::ffff:999.1.1.1', '1::2::3'])(
    'calls %s unparseable',
    (input) => {
      expect(blockedAddressReason(input)).toBe('unparseable address')
    },
  )

  it('treats an unparseable address as blocked, not as public', () => {
    // Fail closed: `null` is the ALLOW answer, so a parse failure must never
    // return null.
    expect(blockedAddressReason('¯\\_(ツ)_/¯')).not.toBeNull()
  })
})

// ── Allowlist parser ────────────────────────────────────────────────────────

describe('TALARIA_FETCH_ALLOW_HOSTS parsing', () => {
  it('allows nothing when unset or empty', () => {
    expect(hostAllowed('anything.example')).toBe(false)
    process.env.TALARIA_FETCH_ALLOW_HOSTS = '   '
    expect(hostAllowed('anything.example')).toBe(false)
  })

  it('matches an exact host, case-insensitively and ignoring a trailing dot', () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = 'mcp.corp.example'
    expect(hostAllowed('mcp.corp.example')).toBe(true)
    expect(hostAllowed('MCP.Corp.Example')).toBe(true)
    expect(hostAllowed('mcp.corp.example.')).toBe(true)
    expect(hostAllowed('other.corp.example')).toBe(false)
    expect(hostAllowed('evil.com/mcp.corp.example')).toBe(false)
  })

  it('matches a *.suffix entry, including the bare domain, without matching a lookalike', () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = '*.corp.example'
    expect(hostAllowed('corp.example')).toBe(true)
    expect(hostAllowed('mcp.corp.example')).toBe(true)
    expect(hostAllowed('a.b.corp.example')).toBe(true)
    // The suffix is '.corp.example' with the dot, so a host that merely ENDS in
    // those characters is not a subdomain of it.
    expect(hostAllowed('evilcorp.example')).toBe(false)
    expect(hostAllowed('corp.example.evil.com')).toBe(false)
  })

  it('accepts comma AND whitespace separated entries', () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = 'a.example, b.example\tc.example\n*.d.example'
    for (const h of ['a.example', 'b.example', 'c.example', 'x.d.example']) expect(hostAllowed(h)).toBe(true)
    expect(hostAllowed('e.example')).toBe(false)
  })

  it('does not treat an address or a CIDR as a hostname', () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = '127.0.0.1, 10.42.0.0/16'
    expect(hostAllowed('127.0.0.1')).toBe(false)
    expect(hostAllowed('10.42.0.0/16')).toBe(false)
  })

  it('re-reads the environment when the spec changes (the cache is keyed on it)', () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = 'first.example'
    expect(hostAllowed('first.example')).toBe(true)
    process.env.TALARIA_FETCH_ALLOW_HOSTS = 'second.example'
    expect(hostAllowed('first.example')).toBe(false)
    expect(hostAllowed('second.example')).toBe(true)
  })
})

// ── assertFetchableUrl ──────────────────────────────────────────────────────

const blocked = async (url: string, match: RegExp) => {
  await expect(assertFetchableUrl(url)).rejects.toThrow(BlockedUrlError)
  await expect(assertFetchableUrl(url)).rejects.toThrow(match)
}

describe('assertFetchableUrl — scheme and shape', () => {
  it.each(['file:///etc/passwd', 'gopher://x/1', 'ftp://x/y', 'jar:http://x!/y', 'data:text/plain,hi'])(
    'refuses %s',
    async (url) => {
      await expect(assertFetchableUrl(url)).rejects.toThrow(BlockedUrlError)
    },
  )

  it('refuses input that is not a URL at all', async () => {
    await blocked('definitely not a url', /not a valid URL/)
    await blocked('', /not a valid URL/)
  })

  it('accepts a URL object as well as a string', async () => {
    const url = new URL('https://example.com/x')
    expect(await assertFetchableUrl(url)).toBe(url)
  })
})

describe('assertFetchableUrl — literal addresses', () => {
  it('refuses loopback, RFC1918, link-local and CGNAT literals', async () => {
    await blocked('http://127.0.0.1:6333/collections', /loopback/)
    await blocked('http://10.0.0.5/', /RFC1918/)
    await blocked('http://192.168.1.1/admin', /RFC1918/)
    await blocked('http://169.254.169.254/latest/meta-data/iam/', /metadata/)
    await blocked('http://100.64.1.1/', /RFC6598/)
  })

  it('refuses IPv6 literals in brackets', async () => {
    await blocked('http://[::1]:8080/', /IPv6 loopback/)
    await blocked('http://[fd00::1]/', /unique-local/)
    await blocked('http://[fe80::1]/', /link-local/)
  })

  it('refuses an IPv4-mapped IPv6 literal', async () => {
    // WHATWG URL normalises this to [::ffff:7f00:1]; the guard must still see
    // 127.0.0.1 through the wrapper.
    await blocked('http://[::ffff:127.0.0.1]/', /loopback/)
  })

  it('refuses the decimal and hex spellings of 127.0.0.1', async () => {
    // The URL parser canonicalises these to 127.0.0.1 before we ever see them,
    // which is exactly why the guard must classify the PARSED hostname.
    await blocked('http://2130706433/', /loopback/)
    await blocked('http://0x7f000001/', /loopback/)
    await blocked('http://127.1/', /loopback/)
  })

  it('allows a public literal without touching DNS', async () => {
    await expect(assertFetchableUrl('https://93.184.216.34/x')).resolves.toBeInstanceOf(URL)
    expect(dnsLookup).not.toHaveBeenCalled()
  })
})

describe('assertFetchableUrl — internal names', () => {
  it.each([
    'http://localhost/',
    'http://LOCALHOST:3000/',
    'http://ip6-localhost/',
    'http://qdrant.internal/',
    'http://printer.local/',
    'http://router.home.arpa/',
    'http://wiki.intranet/',
    'http://nas.lan/',
    'http://foo.localhost/',
  ])('refuses %s by name, before any DNS lookup', async (url) => {
    await expect(assertFetchableUrl(url)).rejects.toThrow(/internal-only hostname/)
    expect(dnsLookup).not.toHaveBeenCalled()
  })
})

describe('assertFetchableUrl — resolved addresses', () => {
  it('refuses a public NAME that resolves into private space', async () => {
    resolvesTo('10.0.0.5')
    await blocked('https://rebind.example/x', /resolves to 10\.0\.0\.5.*RFC1918/)
  })

  it('refuses when ANY of several answers is private (fails closed)', async () => {
    resolvesTo('93.184.216.34', '169.254.169.254')
    await blocked('https://mixed.example/', /169\.254\.169\.254/)
  })

  it('refuses a name resolving to a mapped-v4 loopback', async () => {
    resolvesTo('::ffff:127.0.0.1')
    await blocked('https://sneaky.example/', /loopback/)
  })

  it('allows a name that resolves entirely to public space', async () => {
    resolvesTo('93.184.216.34', '2606:4700:4700::1111')
    await expect(assertFetchableUrl('https://example.com/x')).resolves.toBeInstanceOf(URL)
  })

  it('refuses a name that will not resolve, or that resolves to nothing', async () => {
    dnsLookup.mockRejectedValue(new Error('ENOTFOUND'))
    await blocked('https://nope.example/', /could not resolve/)
    dnsLookup.mockResolvedValue([])
    await blocked('https://empty.example/', /could not resolve/)
  })

  it('strips a trailing dot before resolving', async () => {
    resolvesTo('93.184.216.34')
    await assertFetchableUrl('https://example.com./x')
    expect(dnsLookup.mock.calls[0]?.[0]).toBe('example.com')
  })
})

describe('assertFetchableUrl — operator allowlist', () => {
  it('lets an allowlisted NAME through without any address check', async () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = '*.corp.example'
    await expect(assertFetchableUrl('http://mcp.corp.example/sse')).resolves.toBeInstanceOf(URL)
    expect(dnsLookup).not.toHaveBeenCalled()
  })

  it('lets an allowlisted literal address through', async () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = '127.0.0.1'
    await expect(assertFetchableUrl('http://127.0.0.1:6333/')).resolves.toBeInstanceOf(URL)
    // …and only that one.
    await blocked('http://127.0.0.2/', /loopback/)
  })

  it('honours a CIDR entry for both literals and resolved answers', async () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = '10.42.0.0/16'
    await expect(assertFetchableUrl('http://10.42.1.5/')).resolves.toBeInstanceOf(URL)
    await blocked('http://10.43.1.5/', /RFC1918/)

    resolvesTo('10.42.7.7')
    await expect(assertFetchableUrl('http://mcp.corp.example/')).resolves.toBeInstanceOf(URL)
  })

  it('does not let the allowlist re-open non-http schemes', async () => {
    process.env.TALARIA_FETCH_ALLOW_HOSTS = '*.corp.example'
    await expect(assertFetchableUrl('file://mcp.corp.example/etc/passwd')).rejects.toThrow(/only http and https/)
  })
})

// ── safeFetch: redirects, credential stripping, body cap ────────────────────

/** A fetch stub driven by a queue of canned responses. */
function stubFetch(responses: Array<Response | (() => Response)>) {
  const calls: Array<{ url: string; init: RequestInit }> = []
  const fn = vi.fn(async (input: URL | string, init: RequestInit) => {
    calls.push({ url: String(input), init })
    const next = responses.shift()
    if (!next) throw new Error(`unexpected extra fetch to ${String(input)}`)
    return typeof next === 'function' ? next() : next
  })
  vi.stubGlobal('fetch', fn)
  return calls
}

const redirect = (to: string, status = 302) => new Response(null, { status, headers: { location: to } })
const headerOf = (init: RequestInit, name: string) => new Headers(init.headers as HeadersInit).get(name)

describe('safeFetch — redirect handling', () => {
  beforeEach(() => resolvesTo('93.184.216.34'))

  it('re-validates each hop and refuses a bounce into the metadata service', async () => {
    const calls = stubFetch([redirect('http://169.254.169.254/latest/meta-data/')])
    await expect(safeFetch('https://public.example/start')).rejects.toThrow(/169\.254\.169\.254/)
    expect(calls).toHaveLength(1) // never dialled the second hop
  })

  it('refuses a bounce that only resolves into private space at the next hop', async () => {
    const calls = stubFetch([redirect('https://rebind.example/next')])
    dnsLookup.mockResolvedValueOnce(PUBLIC_V4).mockResolvedValueOnce([{ address: '10.1.2.3', family: 4 }])
    await expect(safeFetch('https://public.example/start')).rejects.toThrow(/10\.1\.2\.3/)
    expect(calls).toHaveLength(1)
  })

  it('follows an allowed redirect and returns the final response', async () => {
    const calls = stubFetch([redirect('https://public.example/final'), new Response('landed', { status: 200 })])
    const res = await safeFetch('https://public.example/start')
    await expect(res.text()).resolves.toBe('landed')
    expect(calls.map((c) => c.url)).toEqual(['https://public.example/start', 'https://public.example/final'])
  })

  it('always drives the chain itself, never letting fetch follow it', async () => {
    const calls = stubFetch([new Response('ok')])
    await safeFetch('https://public.example/x')
    expect(calls[0]?.init.redirect).toBe('manual')
  })

  it('keeps credential headers on a SAME-origin redirect', async () => {
    const calls = stubFetch([redirect('https://public.example/final'), new Response('ok')])
    await safeFetch('https://public.example/start', { headers: { authorization: 'Bearer s3cr3t', 'x-api-key': 'k' } })
    expect(headerOf(calls[1]!.init, 'authorization')).toBe('Bearer s3cr3t')
    expect(headerOf(calls[1]!.init, 'x-api-key')).toBe('k')
  })

  it('strips credential headers on a CROSS-origin redirect', async () => {
    const calls = stubFetch([redirect('https://elsewhere.example/final'), new Response('ok')])
    await safeFetch('https://public.example/start', {
      headers: {
        authorization: 'Bearer s3cr3t',
        cookie: 'session=abc',
        'x-api-key': 'k',
        'x-client-secret': 's',
        accept: 'application/json',
      },
    })
    const sent = new Headers(calls[1]!.init.headers as HeadersInit)
    expect(sent.get('authorization')).toBeNull()
    expect(sent.get('cookie')).toBeNull()
    expect(sent.get('x-api-key')).toBeNull()
    expect(sent.get('x-client-secret')).toBeNull()
    expect(sent.get('accept')).toBe('application/json') // harmless headers survive
  })

  it('treats a port change as cross-origin', async () => {
    const calls = stubFetch([redirect('https://public.example:8443/final'), new Response('ok')])
    await safeFetch('https://public.example/start', { headers: { authorization: 'Bearer s3cr3t' } })
    expect(headerOf(calls[1]!.init, 'authorization')).toBeNull()
  })

  it('downgrades POST to GET and drops the body on a 303 or a 301/302', async () => {
    for (const status of [301, 302, 303]) {
      const calls = stubFetch([redirect('https://public.example/final', status), new Response('ok')])
      await safeFetch('https://public.example/start', {
        method: 'POST',
        body: 'a=1',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
      })
      expect(calls[1]!.init.method).toBe('GET')
      expect(calls[1]!.init.body).toBeUndefined()
      expect(headerOf(calls[1]!.init, 'content-type')).toBeNull()
    }
  })

  it('preserves the method and body across a 307', async () => {
    const calls = stubFetch([redirect('https://public.example/final', 307), new Response('ok')])
    await safeFetch('https://public.example/start', { method: 'POST', body: 'a=1' })
    expect(calls[1]!.init.method).toBe('POST')
    expect(calls[1]!.init.body).toBe('a=1')
  })

  it('resolves a relative Location against the current target', async () => {
    const calls = stubFetch([redirect('/next/page'), new Response('ok')])
    await safeFetch('https://public.example/a/b')
    expect(calls[1]?.url).toBe('https://public.example/next/page')
  })

  it('gives up after maxRedirects', async () => {
    stubFetch(Array.from({ length: 4 }, () => redirect('https://public.example/loop')))
    await expect(safeFetch('https://public.example/start', { maxRedirects: 2 })).rejects.toThrow(/too many redirects/)
  })

  it('returns a 302 with no Location as an ordinary response', async () => {
    stubFetch([new Response('body', { status: 302 })])
    const res = await safeFetch('https://public.example/x')
    expect(res.status).toBe(302)
  })
})

describe('safeFetch — response size cap', () => {
  beforeEach(() => resolvesTo('93.184.216.34'))

  it('refuses a body whose declared Content-Length is over the cap', async () => {
    stubFetch([new Response('short', { headers: { 'content-length': '99999' } })])
    await expect(safeFetch('https://public.example/big', { maxBytes: 1000 })).rejects.toThrow(/declares 99999 bytes/)
  })

  it('refuses a body that exceeds the cap while streaming, without a declared length', async () => {
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.enqueue(new Uint8Array(512))
      },
    })
    stubFetch([new Response(stream)])
    await expect(safeFetch('https://public.example/big', { maxBytes: 1000 })).rejects.toThrow(/exceeded the 1000-byte cap/)
  })

  it('passes a body under the cap through intact', async () => {
    stubFetch([new Response('hello world', { status: 201, statusText: 'Created' })])
    const res = await safeFetch('https://public.example/ok', { maxBytes: 1000 })
    expect(res.status).toBe(201)
    await expect(res.text()).resolves.toBe('hello world')
  })

  it('handles a null-body status without hanging', async () => {
    stubFetch([new Response(null, { status: 204 })])
    const res = await safeFetch('https://public.example/none')
    expect(res.status).toBe(204)
  })

  it('validates the FIRST url before dialling anything', async () => {
    const calls = stubFetch([new Response('never')])
    await expect(safeFetch('http://169.254.169.254/latest/')).rejects.toThrow(BlockedUrlError)
    expect(calls).toHaveLength(0)
  })
})
