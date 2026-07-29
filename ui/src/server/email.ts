// Transactional email — pluggable providers, expanded as requested:
//   smtp    any SMTP server (Google Workspace, etc.) via nodemailer
//   resend  Resend's HTTP API
// Config lives in app_settings with secrets SEALED; the admin panel writes it
// and test-sends. sendEmail is the one entry point every feature uses
// (invites today, more transactional mail later). No provider configured =
// sends fail soft with a clear error the caller can surface.
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
}

/** Send one transactional email through the configured provider. */
export async function sendEmail(input: EmailInput): Promise<{ ok: true } | { ok: false; error: string }> {
  const cfg = await getEmailConfig()
  if (!cfg.provider) return { ok: false, error: 'no email provider configured (Admin → Org → Email)' }
  if (!cfg.from.trim()) return { ok: false, error: 'no From address configured' }
  try {
    if (cfg.provider === 'resend') {
      if (!cfg.resend.apiKeyEnc) return { ok: false, error: 'Resend API key missing' }
      const r = await fetch('https://api.resend.com/emails', {
        method: 'POST',
        headers: { authorization: `Bearer ${open(cfg.resend.apiKeyEnc)}`, 'content-type': 'application/json' },
        body: JSON.stringify({ from: cfg.from, to: [input.to], subject: input.subject, html: input.html, text: input.text }),
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
      connectionTimeout: 15_000,
    })
    await transport.sendMail({ from: cfg.from, to: input.to, subject: input.subject, html: input.html, text: input.text })
    return { ok: true }
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
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
