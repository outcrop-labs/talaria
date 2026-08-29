import { describe, expect, it, vi } from 'vitest'

import { logUpstreamError, sanitizedUpstreamBody, upstreamErrorMessage } from './upstream-error'

// #268: upstream error bodies are upstream-written — hostnames, proxy
// internals, occasionally the credential we sent. These tests pin the one
// door they cross a trust boundary through: status (ours to share), a fixed
// sentence, and the two STRUCTURED tokens OpenAI-style clients switch on for
// retry logic. Everything else free-text dies at the boundary; the verbatim
// body survives only in the log.
describe('upstreamErrorMessage', () => {
  it('carries the status and nothing else', () => {
    expect(upstreamErrorMessage(429)).toBe('upstream error (429)')
    expect(upstreamErrorMessage(500)).not.toContain('nginx')
  })
})

describe('sanitizedUpstreamBody', () => {
  it('keeps structured type/code for retry logic, replaces the message', () => {
    const body = JSON.stringify({
      error: { message: 'Incorrect API key provided: sk-live-9a8b7c. You can find...', type: 'invalid_request_error', code: 'invalid_api_key' },
    })
    const out = JSON.parse(sanitizedUpstreamBody(401, body)) as { error: { message: string; type: string; code: string } }
    expect(out.error.message).toBe('upstream error (401)')
    expect(out.error.type).toBe('invalid_request_error')
    expect(out.error.code).toBe('invalid_api_key')
    expect(sanitizedUpstreamBody(401, body)).not.toContain('sk-live')
  })

  it('drops free text that names infrastructure', () => {
    const body = JSON.stringify({ error: { message: 'connect ECONNREFUSED 10.42.0.7:8443 (proxy.internal.corp)' } })
    const out = sanitizedUpstreamBody(502, body)
    expect(out).not.toContain('10.42.0.7')
    expect(out).not.toContain('proxy.internal')
    expect(JSON.parse(out).error.message).toBe('upstream error (502)')
  })

  it('returns just the fixed sentence for non-JSON bodies (HTML error pages, prose)', () => {
    const out = sanitizedUpstreamBody(502, '<html><body>502 Bad Gateway — nginx/1.24 (upstream gold-prod-3)</body></html>')
    expect(JSON.parse(out)).toEqual({ error: { message: 'upstream error (502)' } })
  })

  it('caps even structured tokens at 64 chars — the fields are upstream-written too', () => {
    const long = 'x'.repeat(200)
    const body = JSON.stringify({ error: { type: long, code: long } })
    const out = JSON.parse(sanitizedUpstreamBody(500, body)) as { error: { type: string; code: string } }
    expect(out.error.type).toHaveLength(64)
    expect(out.error.code).toHaveLength(64)
  })

  it('drops non-string type/code rather than stringifying them', () => {
    const body = JSON.stringify({ error: { type: 42, code: null, message: 'boom' } })
    const out = JSON.parse(sanitizedUpstreamBody(500, body)) as { error: Record<string, unknown> }
    expect(out.error).toEqual({ message: 'upstream error (500)' })
  })
})

describe('logUpstreamError', () => {
  it('is where the verbatim body goes — tagged, and capped', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    try {
      logUpstreamError('llm-v1', 401, 'x'.repeat(900))
      expect(warn).toHaveBeenCalledOnce()
      const [line] = warn.mock.calls[0] as [string]
      expect(line).toContain('[upstream] llm-v1 401:')
      expect(line.length).toBeLessThan(560) // prefix + the 500-char cap
    } finally {
      warn.mockRestore()
    }
  })
})
