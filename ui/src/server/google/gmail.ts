// Gmail service — read the connected user's recent mail (metadata + snippets)
// and send mail on their behalf, acting strictly as that user (per-user OAuth).

import { requireToken } from './connections'

const GMAIL_BASE = 'https://www.googleapis.com/gmail/v1/users/me'
const LABELS_ENDPOINT = `${GMAIL_BASE}/labels`

// ── Labels (Gmail's folders) ─────────────────────────────────────────────────

export interface GmailLabel {
  id: string
  name: string
  type: 'system' | 'user'
}

/** Every label on the account. INBOX and UNREAD are system labels — a message
 *  "is in a folder" by carrying its label, and organizing mail means applying
 *  and removing them. */
export async function listLabelsWithToken(token: string): Promise<GmailLabel[]> {
  const res = await fetch(`${LABELS_ENDPOINT}?fields=labels(id,name,type)`, { headers: { Authorization: `Bearer ${token}` } })
  if (!res.ok) throw new Error(`gmail labels failed: ${res.status} ${await res.text()}`)
  const data = (await res.json()) as { labels?: GmailLabel[] }
  return data.labels ?? []
}

/** Create a label, FIND-OR-CREATE: an existing label of the same name comes
 *  back as-is, so a retry after a timeout is safe and the caller never ends up
 *  with "Vendor" and "vendor " both on the account. */
export async function createLabelWithToken(token: string, name: string): Promise<GmailLabel> {
  const existing = (await listLabelsWithToken(token)).find((l) => l.name === name)
  if (existing) return existing
  const res = await fetch(LABELS_ENDPOINT, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, labelListVisibility: 'labelShow', labelVisibility: 'labelShow' }),
  })
  if (!res.ok) throw new Error(`gmail label create failed: ${res.status} ${await res.text()}`)
  return (await res.json()) as GmailLabel
}

/** Labels an organize call may never touch, on either side. Deleting mail (or
 *  hiding it as spam) is not "organizing" — the toolkit's whole safety story
 *  is that everything it does is reversible, and nothing from TRASH comes back
 *  on its own. */
const FORBIDDEN_LABELS = new Set(['TRASH', 'SPAM', 'BIN'])

export interface OrganizeInput {
  /** Message ids (from the listing tool), up to 100 per call. */
  ids: string[]
  /** Label NAMES to apply (from list_labels / create_label, or INBOX/UNREAD). */
  addLabels?: string[]
  /** Label NAMES to remove — INBOX archives, UNREAD marks read. */
  removeLabels?: string[]
}

/** File/archive/read messages by applying and removing labels. Names are
 *  resolved to ids; an unknown name is an error naming the fix (create_label),
 *  never a silent skip. Nothing here can delete. */
export async function organizeEmailsWithToken(
  token: string,
  input: OrganizeInput,
): Promise<{ updated: number }> {
  const ids = [...new Set(input.ids)].slice(0, 100)
  if (!ids.length) throw new Error('gmail organize: no message ids')
  const add = [...new Set(input.addLabels ?? [])].slice(0, 10)
  const remove = [...new Set(input.removeLabels ?? [])].slice(0, 10)
  for (const name of [...add, ...remove]) {
    if (FORBIDDEN_LABELS.has(name.toUpperCase())) throw new Error(`gmail organize: "${name}" would delete or hide mail — organizing never removes anything from All Mail`)
  }
  const labels = await listLabelsWithToken(token)
  const byName = new Map(labels.map((l) => [l.name, l.id]))
  const addIds = add.map((n) => byName.get(n) ?? missing(n))
  const removeIds = remove.map((n) => byName.get(n) ?? missing(n))
  if (!addIds.length && !removeIds.length) throw new Error('gmail organize: nothing to add or remove')

  // batchModify takes up to 1000 ids; cap the call at 100 (the tool's own cap)
  // so one agent turn cannot reorganize a mailbox wholesale by accident.
  const res = await fetch(`${GMAIL_BASE}/messages/batchModify`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ ids, addLabelIds: addIds, removeLabelIds: removeIds }),
  })
  if (!res.ok) throw new Error(`gmail organize failed: ${res.status} ${await res.text()}`)
  return { updated: ids.length }
}

function missing(name: string): string {
  throw new Error(`gmail organize: no label named "${name}" — create it first (create_label), or spell it as list_labels shows`)
}

/** labelId → name, one labels read for however many ids are asked about. */
async function labelNameMap(token: string, labelIds: string[]): Promise<Map<string, string>> {
  const unique = [...new Set(labelIds)]
  if (!unique.length) return new Map()
  const labels = await listLabelsWithToken(token)
  return new Map(labels.map((l) => [l.id, l.name]))
}

export interface MailSummary {
  id: string
  threadId: string
  from: string
  subject: string
  snippet: string
  date: string | null
  unread: boolean
  /** Label names the message carries (INBOX, UNREAD, and user labels) — what
   *  an organizing agent needs to see before it files anything. */
  labels: string[]
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
  return listRecentMessagesWithToken(await requireToken(userId, nowMs), maxResults, q)
}

/** Recent messages using an already-resolved token (per-user or org). */
export async function listRecentMessagesWithToken(token: string, maxResults = 8, q = 'in:inbox'): Promise<MailSummary[]> {
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

  // One labels read for the whole listing, shared across messages — the label
  // map cannot change between two messages of the same fetch, so asking for it
  // once per message would be N identical calls.
  const labelNames = await labelNameMap(token, msgs.flatMap((m) => m?.labelIds ?? []))

  return msgs
    .filter((m): m is GmailMessage => !!m)
    .map((m) => ({
      id: m.id,
      threadId: m.threadId,
      from: header(m, 'From'),
      subject: header(m, 'Subject') || '(no subject)',
      snippet: m.snippet ?? '',
      date: m.internalDate ? new Date(Number(m.internalDate)).toISOString() : header(m, 'Date') || null,
      unread: (m.labelIds ?? []).includes('UNREAD'),
      labels: (m.labelIds ?? []).map((id) => labelNames.get(id) ?? id),
    }))
}

// ── One full message ─────────────────────────────────────────────────────────

interface GmailPart {
  mimeType?: string
  body?: { data?: string; size?: number }
  parts?: GmailPart[]
}

interface GmailMessageFull {
  id: string
  threadId: string
  snippet?: string
  labelIds?: string[]
  internalDate?: string
  payload?: GmailPart & { headers?: Array<{ name: string; value: string }> }
}

/** Deepest-first part of a given text mime type. */
function textPartOf(part: GmailPart, mime: string): string {
  if (part.mimeType === mime && part.body?.data) return decode(part.body.data)
  for (const child of part.parts ?? []) {
    const found = textPartOf(child, mime)
    if (found) return found
  }
  return ''
}

function decode(b64url: string): string {
  return Buffer.from(b64url, 'base64url').toString('utf8')
}

/** HTML → readable text, hand-rolled (no dependency): an agent reading a
 *  message needs the words, not a browser-grade rendering. Entity order
 *  matters — &amp; last, so a literal "&amp;lt;" cannot decay into "<". */
function htmlToText(html: string): string {
  return html
    .replace(/<(script|style|head|title)\b[^>]*>[\s\S]*?<\/\1\s*>/gi, '')
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<\/(p|div|tr|table|h[1-6]|li|blockquote)\s*>/gi, '\n')
    .replace(/<li\b[^>]*>/gi, '• ')
    .replace(/<[^>]+>/g, '')
    .replace(/&nbsp;/gi, ' ')
    .replace(/&lt;/gi, '<')
    .replace(/&gt;/gi, '>')
    .replace(/&quot;/gi, '"')
    .replace(/&#39;/gi, "'")
    .replace(/&amp;/gi, '&')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

/** The message's own words. text/plain when Google serves one (multipart/
 *  alternative carries the same body without markup — the version to quote),
 *  else the html stripped to text. Html-ONLY mail is common — most
 *  transactional and marketing senders ship no plain part — and returning
 *  empty there was a real read_email failure in the field (body empty,
 *  snippet present). */
function bodyTextOf(part: GmailPart): string {
  const plain = textPartOf(part, 'text/plain')
  if (plain) return plain
  const html = textPartOf(part, 'text/html')
  return html ? htmlToText(html) : ''
}

export interface MailMessage {
  id: string
  threadId: string
  from: string
  to: string
  subject: string
  snippet: string
  date: string | null
  unread: boolean
  /** Label names the message carries (INBOX, UNREAD, and user labels). */
  labels: string[]
  /** The plain-text body — the html stripped to text when the mail ships no
   *  plain part. Empty only when Google serves neither; snippet then. */
  body: string
}

/** One full message (headers + plain-text body) using an already-resolved
 *  token. Body is capped — a mailing-list monster must not eat the agent's
 *  context window whole. */
export async function getMessageWithToken(token: string, id: string): Promise<MailMessage> {
  const r = await fetch(`${GMAIL_BASE}/messages/${encodeURIComponent(id)}?format=full`, {
    headers: { Authorization: `Bearer ${token}` },
  })
  if (!r.ok) throw new Error(`gmail get failed: ${r.status} ${await r.text()}`)
  const m = (await r.json()) as GmailMessageFull
  const h = (name: string) => m.payload?.headers?.find((x) => x.name.toLowerCase() === name.toLowerCase())?.value ?? ''
  const labelIds = (m as { labelIds?: string[] }).labelIds ?? []
  const labelNames = await labelNameMap(token, labelIds)
  return {
    id: m.id,
    threadId: m.threadId,
    from: h('From'),
    to: h('To'),
    subject: h('Subject') || '(no subject)',
    snippet: m.snippet ?? '',
    date: m.internalDate ? new Date(Number(m.internalDate)).toISOString() : h('Date') || null,
    unread: labelIds.includes('UNREAD'),
    labels: labelIds.map((id) => labelNames.get(id) ?? id),
    body: bodyTextOf(m.payload ?? {}).slice(0, 20_000),
  }
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
  return sendMessageWithToken(await requireToken(userId, nowMs), input)
}

/** Send using an already-resolved token (per-user or org). `from` sets a verified
 *  send-as alias on the account; omit to send from the account's own address. */
export async function sendMessageWithToken(token: string, input: SendInput, from?: string | null): Promise<{ id: string; threadId: string }> {
  const headers = [`To: ${input.to}`]
  if (from) headers.push(`From: ${from}`)
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
