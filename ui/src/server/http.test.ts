// The writeHead conversion, locked: Set-Cookie is the one header that
// legitimately repeats (the OAuth login callback answers session-on +
// state-off), and the obvious Object.fromEntries one-liner keeps only the
// last value — the bug that ate Google login on every deployment while
// password login (one cookie) worked.
import { describe, expect, it } from 'vitest'
import { json, writeHeadHeaders } from './http'

describe('writeHeadHeaders', () => {
  it('keeps EVERY Set-Cookie when a response carries several', () => {
    const res = new Response(null, {
      status: 302,
      headers: [
        ['location', '/'],
        ['set-cookie', 'talaria_session=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800'],
        ['set-cookie', 'talaria_oauth_state=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0'],
      ],
    })
    const out = writeHeadHeaders(res)
    // An array, in order — the shape res.writeHead emits one header line per
    // element of. A plain string here means one cookie was lost.
    expect(out['set-cookie']).toEqual([
      'talaria_session=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800',
      'talaria_oauth_state=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0',
    ])
    expect(out['location']).toBe('/')
  })

  it('passes single-cookie responses through as a one-element array', () => {
    const res = json({ ok: true }, { headers: { 'set-cookie': 'talaria_session=sid; Path=/' } })
    expect(writeHeadHeaders(res)['set-cookie']).toEqual(['talaria_session=sid; Path=/'])
    expect(writeHeadHeaders(res)['content-type']).toBe('application/json')
  })

  it('leaves no set-cookie key when the response carries none', () => {
    const out = writeHeadHeaders(json({ user: null }))
    expect('set-cookie' in out).toBe(false)
    expect(out['content-type']).toBe('application/json')
  })
})
