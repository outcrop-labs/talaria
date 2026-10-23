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

  // The answers below are serde_json 1.0.151's (the api's Cargo.lock pin), read
  // off `from_slice::<Value>` and the gateway's `error_byte_offset` for the
  // same bytes. Each is what JSON.parse alone gets wrong.
  /** serde's sentence for a body, as the -32700 must carry it, and the offset. */
  const serde = (body: Uint8Array, message: string, line: number, column: number, offset: number) => {
    const label = new TextDecoder().decode(body).slice(0, 60)
    expect(jsonSyntaxError(body), label).toEqual({ message, offset })
    const msg = refusal(body)
    expect(msg, label).toContain(`not valid JSON: ${message} at line ${line} column ${column}.`)
    expect(msg, label).toContain(`offset ${offset} |`)
  }

  it('nesting past serde’s recursion limit is refused at the 128th array or object, as the Rust gateway refuses it', () => {
    expect(readRpcBody(bytes('['.repeat(127) + ']'.repeat(127))).ok).toBe(true)
    expect(readRpcBody(bytes('{"a":'.repeat(127) + '1' + '}'.repeat(127))).ok).toBe(true)
    serde(bytes('['.repeat(128) + ']'.repeat(128)), 'recursion limit exceeded', 1, 128, 128)
    serde(bytes('['.repeat(128)), 'recursion limit exceeded', 1, 128, 128)
    serde(bytes('{"a":'.repeat(128) + '1' + '}'.repeat(128)), 'recursion limit exceeded', 1, 636, 636)
    serde(bytes('[{"a":'.repeat(64) + '1' + '}]'.repeat(64)), 'recursion limit exceeded', 1, 380, 380)
    const deepArgument = `{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":{"v":${'['.repeat(130)}${']'.repeat(130)}}}}`
    serde(bytes(deepArgument), 'recursion limit exceeded', 1, 209, 209)
  })

  it('a lone surrogate escape is refused, in serde’s words and at serde’s offset; a pair parses', () => {
    serde(bytes('"\\ud800"'), 'unexpected end of hex escape', 1, 8, 8)
    serde(bytes('"\\ud800x"'), 'unexpected end of hex escape', 1, 8, 8)
    serde(bytes('"\\ud800\\n"'), 'unexpected end of hex escape', 1, 9, 9)
    serde(bytes('"\\udc00"'), 'lone leading surrogate in hex escape', 1, 7, 7)
    serde(bytes('"\\ud800\\u0041"'), 'lone leading surrogate in hex escape', 1, 13, 13)
    serde(bytes('"\\ud800\\ud800"'), 'lone leading surrogate in hex escape', 1, 13, 13)
    serde(bytes('"\\ud800\\uzzzz"'), 'invalid escape', 1, 13, 13)
    serde(bytes('{"\\ud800":1}'), 'unexpected end of hex escape', 1, 9, 9)
    serde(bytes('"\\ud800'), 'EOF while parsing a string', 1, 7, 7)
    serde(bytes('"\\ud800\\'), 'EOF while parsing a string', 1, 8, 8)
    serde(bytes('"\\ud800\\u00'), 'EOF while parsing a string', 1, 11, 11)
    // A tools/call whose argument carries half an emoji never reaches a handler.
    const halfEmoji = '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":{"q":"a\\ud83dz"}}}'
    serde(bytes(halfEmoji), 'unexpected end of hex escape', 1, 93, 93)
    for (const ok of ['"\\ud800\\udc00"', '"\\uDBFF\\uDFFF"', '"\\ud83d\\ude00"']) {
      const r = readRpcBody(bytes(ok))
      expect(r.ok, ok).toBe(true)
    }
  })

  it('an overlong form, an encoded surrogate or a code point past U+10FFFF is refused where serde refuses it', () => {
    serde(cat('"a', [0xe0, 0x80, 0x80], 'b"'), 'invalid unicode code point', 1, 7, 7)
    serde(cat('"a', [0xed, 0xa0, 0x80], 'b"'), 'invalid unicode code point', 1, 7, 7)
    serde(cat('"', [0xf0, 0x80, 0x80, 0x80], '"'), 'invalid unicode code point', 1, 6, 6)
    serde(cat('"', [0xf4, 0x90, 0x80, 0x80], '"'), 'invalid unicode code point', 1, 6, 6)
    serde(cat('"', [0xe0, 0x80], '\\n', [0x80], '"'), 'invalid unicode code point', 1, 7, 7)
    serde(cat('{"a":1,\n"k', [0xed, 0xa0, 0x80], '":2}'), 'invalid unicode code point', 2, 6, 14)
    // The edges of each narrowed range are UTF-8.
    for (const ok of [[0xe0, 0xa0, 0x80], [0xed, 0x9f, 0xbf], [0xf0, 0x90, 0x80, 0x80], [0xf4, 0x8f, 0xbf, 0xbf]]) {
      expect(readRpcBody(cat('"', ok, '"')).ok, ok.join(' ')).toBe(true)
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
