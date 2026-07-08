// Gmail service — read the connected user's recent mail (metadata + snippets)
// and send mail on their behalf, acting strictly as that user (per-user OAuth).

import { getAccessToken } from './connections'

const GMAIL_BASE = 'https://www.googleapis.com/gmail/v1/users/me'

async function requireToken(userId: string, nowMs: number): Promise<string> {
  const token = await getAccessToken(userId, nowMs)
  if (!token) {
    const err = new Error('not_connected')
    err.name = 'GoogleNotConnected'
    throw err
  }
  return token
}

export interface MailSummary {
  id: string
  threadId: string
  from: string
  subject: string
  snippet: string
  date: string | null
  unread: boolean
}

interface GmailMessage {
  id: string
  threadId: string
  snippet?: string
  labelIds?: string[]
  internalDate?: string
  payload?: { headers?: Array<{ name: string; value: string }> }
}

const header = (m: GmailMessage, name: string): string =>
  m.payload?.headers?.find((h) => h.name.toLowerCase() === name.toLowerCase())?.value ?? ''

/** Recent messages (metadata only), newest first. `q` is Gmail search syntax. */
export async function listRecentMessages(userId: string, nowMs: number, maxResults = 8, q = 'in:inbox'): Promise<MailSummary[]> {
  const token = await requireToken(userId, nowMs)
  const auth = { Authorization: `Bearer ${token}` }

  const listParams = new URLSearchParams({ maxResults: String(Math.min(Math.max(maxResults, 1), 25)), q })
  const listRes = await fetch(`${GMAIL_BASE}/messages?${listParams.toString()}`, { headers: auth })
  if (!listRes.ok) throw new Error(`gmail list failed: ${listRes.status} ${await listRes.text()}`)
  const list = (await listRes.json()) as { messages?: Array<{ id: string }> }
  const ids = (list.messages ?? []).map((m) => m.id)

  // Fetch each message's metadata (headers + snippet) in parallel.
  const msgs = await Promise.all(
    ids.map(async (id) => {
      const p = new URLSearchParams({ format: 'metadata' })
      for (const h of ['From', 'Subject', 'Date']) p.append('metadataHeaders', h)
      const r = await fetch(`${GMAIL_BASE}/messages/${id}?${p.toString()}`, { headers: auth })
      if (!r.ok) return null
      return (await r.json()) as GmailMessage
    }),
  )

  return msgs.filter((m): m is GmailMessage => !!m).map((m) => ({
    id: m.id,
    threadId: m.threadId,
    from: header(m, 'From'),
    subject: header(m, 'Subject') || '(no subject)',
    snippet: m.snippet ?? '',
    date: m.internalDate ? new Date(Number(m.internalDate)).toISOString() : header(m, 'Date') || null,
    unread: (m.labelIds ?? []).includes('UNREAD'),
  }))
}

export interface SendInput {
  to: string
  subject: string
  body: string
  cc?: string
  bcc?: string
}

/** RFC 2047 encode a header value if it has non-ASCII characters. */
function encodeHeader(value: string): string {
  // eslint-disable-next-line no-control-regex
  if (/^[\x00-\x7F]*$/.test(value)) return value
  return `=?UTF-8?B?${Buffer.from(value, 'utf8').toString('base64')}?=`
}

/** Send a plain-text email as the connected user. Returns the sent message id. */
export async function sendMessage(userId: string, nowMs: number, input: SendInput): Promise<{ id: string; threadId: string }> {
  const token = await requireToken(userId, nowMs)

  const headers = [`To: ${input.to}`]
  if (input.cc) headers.push(`Cc: ${input.cc}`)
  if (input.bcc) headers.push(`Bcc: ${input.bcc}`)
  headers.push(
    `Subject: ${encodeHeader(input.subject)}`,
    'MIME-Version: 1.0',
    'Content-Type: text/plain; charset="UTF-8"',
    'Content-Transfer-Encoding: 8bit',
  )
  // Headers, a blank separator line, then the body.
  const mime = `${headers.join('\r\n')}\r\n\r\n${input.body}`
  const raw = Buffer.from(mime, 'utf8').toString('base64url')

  const res = await fetch(`${GMAIL_BASE}/messages/send`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ raw }),
  })
  if (!res.ok) throw new Error(`gmail send failed: ${res.status} ${await res.text()}`)
  const sent = (await res.json()) as { id: string; threadId: string }
  return { id: sent.id, threadId: sent.threadId }
}
