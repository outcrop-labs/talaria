import { afterEach, describe, expect, it, vi } from 'vitest'
import { getMessageWithToken } from '@/server/google/gmail'

// THE FULL-MESSAGE READ, and the shape under test throughout is what the model
// actually receives: Gmail answers `format=full` with a nested MIME tree whose
// leaves are base64url strings. Every assertion here pins a shape a provider
// really sends, because each one has broken a naive parser somewhere:
//   - multipart/alternative with plain + html versions of the same body
//   - a plain part nested one level deeper than the alternative node
//   - non-ASCII text that must round-trip the base64url decode as UTF-8

const b64 = (s: string) => Buffer.from(s, 'utf8').toString('base64url')

const message = (payload: unknown) =>
  new Response(
    JSON.stringify({
      id: 'm-1',
      threadId: 't-1',
      snippet: 'the teaser',
      labelIds: ['UNREAD'],
      internalDate: '1755000000000',
      payload,
    }),
    { status: 200 },
  )

const headers = [
  { name: 'From', value: 'priya@example.com' },
  { name: 'To', value: 'me@example.com' },
  { name: 'Subject', value: 'Vendor key' },
]

afterEach(() => vi.unstubAllGlobals())

describe('getMessageWithToken', () => {
  it('decodes the plain-text body from a multipart/alternative tree', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        message({
          headers,
          mimeType: 'multipart/alternative',
          parts: [
            { mimeType: 'text/plain', body: { data: b64('Legal signed off.\n\nOne catch: staging only until Monday.') } },
            { mimeType: 'text/html', body: { data: b64('<p>Legal signed off.</p>') } },
          ],
        }),
      ),
    )
    const m = await getMessageWithToken('tok', 'm-1')
    // PLAIN WINS over html: it is the same message without markup, and the
    // version an agent should quote back.
    expect(m.body).toBe('Legal signed off.\n\nOne catch: staging only until Monday.')
    expect(m.from).toBe('priya@example.com')
    expect(m.to).toBe('me@example.com')
    expect(m.subject).toBe('Vendor key')
    expect(m.unread).toBe(true)
  })

  it('finds a plain part nested deeper than the top node', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        message({
          headers,
          mimeType: 'multipart/mixed',
          parts: [
            {
              mimeType: 'multipart/alternative',
              parts: [
                { mimeType: 'text/plain', body: { data: b64('nested words') } },
                { mimeType: 'text/html', body: { data: b64('<p>nested words</p>') } },
              ],
            },
          ],
        }),
      ),
    )
    const m = await getMessageWithToken('tok', 'm-1')
    expect(m.body).toBe('nested words')
  })

  it('round-trips non-ASCII text as UTF-8', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => message({ headers, mimeType: 'text/plain', body: { data: b64('Café — résumé 📎') } })),
    )
    const m = await getMessageWithToken('tok', 'm-1')
    expect(m.body).toBe('Café — résumé 📎')
  })

  it('answers an empty body when Google serves no text part — the snippet then carries it', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => message({ headers, mimeType: 'text/html', body: { data: b64('<p>only html</p>') } })),
    )
    const m = await getMessageWithToken('tok', 'm-1')
    expect(m.body).toBe('')
    expect(m.snippet).toBe('the teaser')
  })

  it('caps a monstrous body — a mailing-list digest must not eat the context window whole', async () => {
    const huge = 'x'.repeat(50_000)
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => message({ headers, mimeType: 'text/plain', body: { data: b64(huge) } })),
    )
    const m = await getMessageWithToken('tok', 'm-1')
    expect(m.body.length).toBe(20_000)
  })

  it('surfaces a Google refusal as a thrown error the route maps, never a silent empty', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('{"error": "404"}', { status: 404 })))
    await expect(getMessageWithToken('tok', 'gone')).rejects.toThrow('gmail get failed: 404')
  })
})
