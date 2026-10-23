import { describe, expect, it } from 'vitest'
import {
  BATCH_NOT_SERVED,
  BATCH_REFUSED,
  batchAnswer,
  invalidRequest,
  jsonSyntaxError,
  methodClosedBySubset,
  methodRefusal,
  parseDiagnostic,
  readRpcBody,
  recordedMembers,
  stripBom,
  toolOf,
} from './mcp-jsonrpc'

// THE QUESTION THIS FILE PINS DOWN: does the app gateway answer a body it
// will not dispatch the way the Rust gateway does? Each case here is a twin of
// a test in api/crates/talaria-routes-fleet/src/mcp/mcp_gw_server.rs — the same
// bodies, the same codes, the same offsets and the same bounds — so an agent
// gets one contract whichever runtime answers its server.

const bytes = (s: string): Uint8Array => new TextEncoder().encode(s)
const cat = (...parts: Array<string | number[]>): Uint8Array =>
  new Uint8Array(parts.flatMap((p) => (typeof p === 'string' ? [...bytes(p)] : p)))

/** The -32700 sentence for a body, as the gateway answers it. */
const refusal = (body: Uint8Array): string => {
  const r = readRpcBody(body)
  if (r.ok) throw new Error('parsed')
  return r.message
}

describe('a body that is not JSON', () => {
  it('a raw control byte in a string is named, with the 0a in the dump', () => {
    const body = cat('{"method":"tools/call","params":{"name":"x","arg":"a', [0x0a], 'b"}}')
    const msg = refusal(body)
    expect(msg).toContain('control character')
    expect(msg).toContain(' 0a ')
    expect(msg.length).toBeLessThan(450)
  })

  it('three braces report the offset and a bounded dump', () => {
    const body = bytes('{{{')
    expect(jsonSyntaxError(body)).toEqual({ message: 'key must be a string', offset: 2 })
    expect(refusal(body)).toContain('offset 2')
    expect(parseDiagnostic(body, 2)).toBe('offset 2 | 7b 7b 7b | {{{')
  })

  it('an empty body, a bare word, SSE framing and blank space are each a parse error', () => {
    for (const b of ['', 'ping', 'data: {}\n\n', '  \n']) {
      const r = readRpcBody(bytes(b))
      expect(r.ok, JSON.stringify(b)).toBe(false)
    }
    expect(jsonSyntaxError(bytes(''))).toEqual({ message: 'EOF while parsing a value', offset: 0 })
  })

  it('a BOM is stripped, so a well-formed body parses and a bad one is still refused', () => {
    const good = cat([0xef, 0xbb, 0xbf], '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete_everything"}}')
    const r = readRpcBody(good)
    expect(r.ok && toolOf(r.value)).toBe('delete_everything')
    expect(readRpcBody(cat([0xef, 0xbb, 0xbf], '{{{')).ok).toBe(false)
    expect([...stripBom(bytes('{}'))]).toEqual([...bytes('{}')])
    expect([...stripBom(new Uint8Array([0xef, 0xbb]))]).toEqual([0xef, 0xbb])
  })

  it('a markdown ticket body with raw newlines carries the caller-side hint', () => {
    const body = cat(
      '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"create_ticket","arguments":{"description":"## What\'s broken - the gateway rejected a valid filing.',
      [0x0a, 0x0a],
      '## Evidence and fix"}}',
    )
    const msg = refusal(body)
    expect(msg).toContain('control character')
    expect(msg).toContain('the fix is on the sender')
    expect(msg).toContain(' 0a ')
    expect(msg.length).toBeLessThan(450)
  })

  it('a unicode payload never leaks its text into the answer', () => {
    const body = cat('{"method":"tools/call","params":{"name":"translate","text":"café', [0x07], '中文 🦀"}}')
    const msg = refusal(body)
    expect(msg).toContain('control character')
    for (const text of ['café', '中文', '🦀']) expect(msg).not.toContain(text)
    expect(msg.length).toBeLessThan(450)
  })

  it('a multi-kB body still gets a bounded answer; an end-of-input failure points at the end', () => {
    const body = bytes(`{"method":"tools/call","params":{"note":"${'7'.repeat(12_000)}"`)
    const msg = refusal(body)
    expect(msg.length).toBeLessThan(450)
    expect(msg).not.toContain('7'.repeat(16))
    expect(msg).toContain(`offset ${body.length} |`)
  })

  it('the offset walks to a failure on a later line', () => {
    const body = bytes('{"a":1,\n"b":2,,}')
    const msg = refusal(body)
    expect(msg).toContain('offset 15 |')
    expect(msg).toContain('at line 2 column 7')
    expect(msg).toContain(' 0a ')
  })

  it('names serde’s other failures in serde’s words (each checked against the Rust gateway live)', () => {
    const cases: Array<[string, string]> = [
      ['[1,]', 'trailing comma'],
      ['{"a":1,}', 'trailing comma'],
      ['{"a" 1}', 'expected `:`'],
      ['{"a":1 "b":2}', 'expected `,` or `}`'],
      ['[1 2]', 'expected `,` or `]`'],
      ['"\\x"', 'invalid escape'],
      ['01', 'invalid number'],
      ['-', 'EOF while parsing a value'],
      ['1.', 'EOF while parsing a value'],
      ['1.e', 'invalid number'],
      ['tru', 'EOF while parsing a value'],
      ['trve', 'expected ident'],
      ['{"a":1} x', 'trailing characters'],
      ['{"a":', 'EOF while parsing a value'],
      ['{"a"', 'EOF while parsing an object'],
      ['["a"', 'EOF while parsing a list'],
      ['"abc', 'EOF while parsing a string'],
      ['?', 'expected value'],
    ]
    for (const [body, message] of cases) expect(jsonSyntaxError(bytes(body))?.message, body).toBe(message)
    // Where serde says it broke, not only what: these offsets are the Rust
    // gateway's, read off its answers to the same bodies.
    const at: Array<[Uint8Array, string, number]> = [
      [bytes('"\\u12"'), 'EOF while parsing a string', 6],
      [bytes('"\\u12x4"'), 'invalid escape', 7],
      [bytes('"\\uzzzz"'), 'invalid escape', 7],
      [bytes('[ '), 'EOF while parsing a list', 2],
      [bytes('{ '), 'EOF while parsing an object', 2],
      [cat('"a', [0xff], 'bcdef"'), 'invalid unicode code point', 9],
      [cat('{"k":"a', [0xc3], 'bcd","z":1}'), 'invalid unicode code point', 12],
      [cat('"ab', [0xe2, 0x82], '"'), 'invalid unicode code point', 6],
      [bytes('-01'), 'invalid number', 3],
    ]
    for (const [body, message, offset] of at) expect(jsonSyntaxError(body), new TextDecoder().decode(body)).toEqual({ message, offset })
    for (const ok of ['{}', '[]', '{"a":[1,-2.5e+3,true,false,null,"\\u00e9\\n"]}', ' 0 ', '"é"']) {
      expect(jsonSyntaxError(bytes(ok)), ok).toBeNull()
    }
  })
})

describe('a body that parsed but is not one request', () => {
  it('a batch carrying a tools/call is refused per member, with the Rust sentence', () => {
    const batch = [
      { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'delete_repo' } },
      { jsonrpc: '2.0', method: 'notifications/initialized' },
      { jsonrpc: '2.0', id: 'b', method: 'tools/list' },
    ]
    expect(batchAnswer(batch)).toEqual([
      { jsonrpc: '2.0', id: 1, error: { code: -32600, message: BATCH_REFUSED } },
      { jsonrpc: '2.0', id: 'b', error: { code: -32600, message: BATCH_REFUSED } },
    ])
    expect(recordedMembers(batch).map(toolOf)).toEqual(['delete_repo'])
    // A batch of id-less calls still gets an answer, with a null id.
    expect(batchAnswer([{ jsonrpc: '2.0', method: 'tools/call', params: { name: 'x' } }])).toEqual({
      jsonrpc: '2.0',
      id: null,
      error: { code: -32600, message: BATCH_REFUSED },
    })
    // Chatter in a batch has nothing to be passed to here: refused too.
    expect(batchAnswer([{ jsonrpc: '2.0', id: 1, method: 'ping' }])).toEqual([
      { jsonrpc: '2.0', id: 1, error: { code: -32600, message: BATCH_NOT_SERVED } },
    ])
    expect(batchAnswer([])).toEqual({ jsonrpc: '2.0', id: null, error: { code: -32600, message: BATCH_NOT_SERVED } })
  })

  it('a non-object or a method-less object is an invalid request; a request is not', () => {
    for (const v of [42, 'x', null, true]) expect(invalidRequest(v)).toEqual({ jsonrpc: '2.0', id: null, error: { code: -32600, message: 'the request is not a JSON-RPC request object' } })
    expect(invalidRequest({ id: 3 })).toEqual({ jsonrpc: '2.0', id: 3, error: { code: -32600, message: 'the request names no method' } })
    expect(invalidRequest({ id: 3, method: 7 })).toEqual({ jsonrpc: '2.0', id: 3, error: { code: -32600, message: 'the request names no method' } })
    expect(invalidRequest({ jsonrpc: '2.0', method: 'ping' })).toBeNull()
  })

  it('only a single tools/call or a batch’s tools/call members are recorded; a tool is its string name', () => {
    expect(recordedMembers({ method: 'tools/call', params: { name: 'a' } })).toHaveLength(1)
    expect(recordedMembers({ method: 'tools/list' })).toEqual([])
    expect(recordedMembers(undefined)).toEqual([])
    expect(toolOf({ params: { name: 42 } })).toBeNull()
    expect(toolOf({ params: {} })).toBeNull()
  })

  it('a tool subset closes resources, prompts and completion — nothing else', () => {
    for (const m of ['resources/read', 'resources/list', 'resources/templates/list', 'prompts/get', 'prompts/list', 'completion/complete']) {
      expect(methodClosedBySubset(m), m).toBe(true)
    }
    for (const m of ['initialize', 'ping', 'notifications/initialized', 'logging/setLevel', 'tools/list', 'tools/call']) {
      expect(methodClosedBySubset(m), m).toBe(false)
    }
    expect(methodRefusal('prompts/get')).toBe('method "prompts/get" is not available to this agent here (it holds a tool subset)')
  })
})
