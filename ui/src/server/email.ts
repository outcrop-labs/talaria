// Transactional email — pluggable providers, expanded as requested:
//   smtp    any SMTP server (Google Workspace, etc.) via nodemailer
//   resend  Resend's HTTP API
// Config lives in app_settings with secrets SEALED; the admin panel writes it
// and test-sends. sendEmail is the one entry point every feature uses
// (invites and notification routing today, more transactional mail later). No
// provider configured = sends fail soft with a clear error the caller can
// surface.
//
// sendEmail NEVER THROWS AND ALWAYS RETURNS. Both halves matter to its callers:
// most sends are a side effect of somebody else's request or of a queue drain,
// so a provider that is merely slow must become a reported failure rather than
// a caller that waits. Every phase is bounded, and the whole call is bounded
// again by EMAIL_SEND_TIMEOUT_MS on top.
import { getSetting, setSetting } from './audit'
import { seal, open } from './secretbox'

export interface EmailConfig {
  provider: 'smtp' | 'resend' | null
  /** From header, e.g. "Talaria <talaria@yourcompany.com>". */
  from: string
  smtp: { host: string; port: number; secure: boolean; user: string; passEnc: string | null }
  resend: { apiKeyEnc: string | null }
}

const KEY = 'email_config'
const DEFAULTS: EmailConfig = {
  provider: null,
  from: '',
  smtp: { host: '', port: 587, secure: false, user: '', passEnc: null },
  resend: { apiKeyEnc: null },
}

export const getEmailConfig = async (): Promise<EmailConfig> => ({
  ...DEFAULTS,
  ...(await getSetting<Partial<EmailConfig>>(KEY, {})),
})

export async function setEmailConfig(patch: {
  provider?: 'smtp' | 'resend' | null
  from?: string
  smtp?: { host?: string; port?: number; secure?: boolean; user?: string; pass?: string | null }
  resend?: { apiKey?: string | null }
}): Promise<void> {
  const cur = await getEmailConfig()
  const next: EmailConfig = {
    provider: patch.provider !== undefined ? patch.provider : cur.provider,
    from: patch.from !== undefined ? patch.from : cur.from,
    smtp: {
      host: patch.smtp?.host ?? cur.smtp.host,
      port: patch.smtp?.port ?? cur.smtp.port,
      secure: patch.smtp?.secure ?? cur.smtp.secure,
      user: patch.smtp?.user ?? cur.smtp.user,
      // undefined = keep; null = clear; string = replace (sealed).
      passEnc: patch.smtp?.pass === undefined ? cur.smtp.passEnc : patch.smtp.pass === null ? null : seal(patch.smtp.pass),
    },
    resend: {
      apiKeyEnc: patch.resend?.apiKey === undefined ? cur.resend.apiKeyEnc : patch.resend.apiKey === null ? null : seal(patch.resend.apiKey),
    },
  }
  await setSetting(KEY, next)
}

export interface EmailInput {
  to: string
  subject: string
  html: string
  text?: string
  /** Extra RFC 5322 headers. One reason it exists: `List-Unsubscribe`, which
   *  every bulk-ish mail we send needs — a mail client that can offer a
   *  one-tap unsubscribe is a mail client that does not offer "report spam"
   *  instead, and the reputation of the sending domain is the whole product's
   *  ability to reach anybody. Both providers below carry it verbatim. */
  headers?: Record<string, string>
}

/** The outside edge of one send attempt, whatever the provider does.
 *
 *  The per-phase timeouts below (connection, greeting, socket, the fetch abort)
 *  each bound one step; none of them bounds the WHOLE call, and the failure
 *  that matters is the one where every individual step keeps just barely
 *  making progress. A caller draining a queue needs to know that `sendEmail`
 *  returns — an unbounded send is how a slow provider turns into a growing
 *  backlog instead of a reported error. */
export const EMAIL_SEND_TIMEOUT_MS = 30_000

/** Resolve to a failure if `work` has not settled in `ms`. The work itself is
 *  not cancellable (nodemailer's promise has no abort), so this is a deadline
 *  on the CALLER, not on the socket — the point is that the queue moves on. Its
 *  eventual rejection is swallowed deliberately: it belongs to a send we have
 *  already reported as timed out, and an unhandled rejection here would take
 *  down a process for an email. */
async function withDeadline<T>(work: Promise<T>, ms: number, what: string): Promise<T | { ok: false; error: string }> {
  let bell: ReturnType<typeof setTimeout> | undefined
  const deadline = new Promise<{ ok: false; error: string }>((resolve) => {
    bell = setTimeout(() => resolve({ ok: false, error: `${what} did not respond within ${Math.round(ms / 1000)}s` }), ms)
  })
  try {
    return await Promise.race([work.catch((e: unknown) => ({ ok: false as const, error: (e as Error).message })), deadline])
  } finally {
    clearTimeout(bell)
  }
}

/** Send one transactional email through the configured provider. Always
 *  returns — see EMAIL_SEND_TIMEOUT_MS — and never throws. */
export async function sendEmail(input: EmailInput): Promise<{ ok: true } | { ok: false; error: string }> {
  const cfg = await getEmailConfig().catch((e: unknown) => {
    // Reading the config is a database round trip, and a send is a side effect
    // of somebody else's work. Report it like any other send failure.
    console.error('[email] could not read the email config:', e)
    return null
  })
  if (!cfg) return { ok: false, error: 'could not read the email configuration' }
  if (!cfg.provider) return { ok: false, error: 'no email provider configured (Admin → Org → Email)' }
  if (!cfg.from.trim()) return { ok: false, error: 'no From address configured' }
  return withDeadline(sendVia(cfg, input), EMAIL_SEND_TIMEOUT_MS, cfg.provider) as Promise<
    { ok: true } | { ok: false; error: string }
  >
}

async function sendVia(cfg: EmailConfig, input: EmailInput): Promise<{ ok: true } | { ok: false; error: string }> {
  try {
    if (cfg.provider === 'resend') {
      if (!cfg.resend.apiKeyEnc) return { ok: false, error: 'Resend API key missing' }
      const r = await fetch('https://api.resend.com/emails', {
        method: 'POST',
        headers: { authorization: `Bearer ${open(cfg.resend.apiKeyEnc)}`, 'content-type': 'application/json' },
        body: JSON.stringify({
          from: cfg.from,
          to: [input.to],
          subject: input.subject,
          html: input.html,
          text: input.text,
          ...(input.headers && Object.keys(input.headers).length ? { headers: input.headers } : {}),
        }),
        signal: AbortSignal.timeout(15_000),
      })
      if (!r.ok) {
        const j = (await r.json().catch(() => ({}))) as { message?: string }
        return { ok: false, error: `Resend: ${j.message ?? r.status}` }
      }
      return { ok: true }
    }
    // smtp
    if (!cfg.smtp.host || !cfg.smtp.user) return { ok: false, error: 'SMTP host/user missing' }
    const { default: nodemailer } = await import('nodemailer')
    const transport = nodemailer.createTransport({
      host: cfg.smtp.host,
      port: cfg.smtp.port,
      secure: cfg.smtp.secure,
      auth: { user: cfg.smtp.user, pass: cfg.smtp.passEnc ? open(cfg.smtp.passEnc) : '' },
      // Three phases, three bounds. `connectionTimeout` alone leaves the two
      // ways an SMTP server hangs while still connected — accepting the TCP
      // connection and never greeting, or greeting and then going quiet
      // mid-dialogue — completely unbounded.
      connectionTimeout: 10_000,
      greetingTimeout: 10_000,
      socketTimeout: 20_000,
    })
    try {
      await transport.sendMail({
        from: cfg.from,
        to: input.to,
        subject: input.subject,
        html: input.html,
        text: input.text,
        headers: input.headers,
      })
      return { ok: true }
    } finally {
      // One transport per send, so it must be closed per send. A queue draining
      // hundreds of mails would otherwise leave a pool of sockets behind for
      // each one, and the first symptom is the file-descriptor limit.
      transport.close()
    }
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
}

/** Escape a string for interpolation into email HTML. Notification titles and
 *  bodies are user- and agent-written text, and they reach a mail client that
 *  will happily render whatever tags survive. */
export function emailEscape(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)
}

/** The one call-to-action button, so every mail's primary link looks the same. */
export function emailButton(href: string, label: string): string {
  return `<p style="margin:24px 0"><a href="${emailEscape(href)}" style="background:#1a1a18;color:#ffffff;text-decoration:none;padding:10px 18px;border-radius:8px;font-size:14px">${emailEscape(label)}</a></p>`
}

/** The shared shell for transactional mail — quiet, dark-agnostic, no images. */
export function emailShell(title: string, bodyHtml: string, footer = 'Sent by Talaria'): string {
  return `<!doctype html><html><body style="margin:0;padding:32px 16px;background:#f6f6f4;font-family:-apple-system,'Segoe UI',Roboto,Helvetica,Arial,sans-serif">
<div style="max-width:520px;margin:0 auto;background:#ffffff;border:1px solid #e5e5e0;border-radius:12px;padding:32px">
<h1 style="margin:0 0 16px;font-size:18px;color:#1a1a18">${title}</h1>
<div style="font-size:14px;line-height:1.6;color:#3a3a36">${bodyHtml}</div>
</div>
<p style="max-width:520px;margin:16px auto 0;font-size:11px;color:#8a8a84;text-align:center">${footer}</p>
</body></html>`
}
