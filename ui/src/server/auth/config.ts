// Auth configuration — resolved from the environment at request time.
//
// Every provider is INDEPENDENTLY enable-able/disable-able. A provider is only
// "enabled" if its flag is on AND its required secrets are present, so a
// half-configured provider never shows up in the login screen.
//
//   AUTH_SECRET                signing key for session + state cookies (required)
//   AUTH_PUBLIC_URL            external origin (for OAuth redirect URIs) — optional,
//                              falls back to the request origin
//   AUTH_ALLOWED_DOMAINS       comma-separated email domains allowed to sign in
//   AUTH_ALLOWED_EMAILS        comma-separated exact emails allowed to sign in
//
//   Google OAuth — credentials from EITHER the Admin UI record (Admin → Org →
//   Google Workspace, stored in app_settings; see google/client-config.ts) OR:
//   AUTH_GOOGLE_CLIENT_ID / AUTH_GOOGLE_CLIENT_SECRET
//   AUTH_GOOGLE_ENABLED=1     login bootstrap — the Admin UI toggle beside the
//                             client is the primary switch; env pins login ON
//                             (see googleLoginEnabled)
//   AUTH_GOOGLE_HD             optional Google Workspace hosted-domain restriction
//
//   Username / password
//   AUTH_PASSWORD_ENABLED=1
//   AUTH_USERS                 "user:pass,user2:pass2" — each password may be
//                              plaintext or a `scrypt$…` hash (see
//                              auth/password.ts; plaintext warns at boot)

export type ProviderId = 'google' | 'password'

export interface ProviderMeta {
  id: ProviderId
  /** Human label for the login button/form. */
  label: string
  /** 'oauth' → redirect flow; 'password' → inline form. */
  kind: 'oauth' | 'password'
}

const flag = (v: string | undefined) => v === '1' || v === 'true'
const list = (v: string | undefined) =>
  (v ?? '')
    .split(',')
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean)

export interface AuthConfig {
  secret: string
  publicUrl: string | null
  allowedDomains: string[]
  allowedEmails: string[]
  google: { enabled: boolean; clientId: string; clientSecret: string; hd: string | null }
  password: { enabled: boolean; users: Array<{ username: string; password: string }> }
}

export function getAuthConfig(): AuthConfig {
  const env = process.env

  const googleEnabled =
    flag(env.AUTH_GOOGLE_ENABLED) && !!env.AUTH_GOOGLE_CLIENT_ID && !!env.AUTH_GOOGLE_CLIENT_SECRET

  const users = (env.AUTH_USERS ?? '')
    .split(',')
    .map((pair) => pair.trim())
    .filter(Boolean)
    .map((pair) => {
      const idx = pair.indexOf(':')
      return idx === -1
        ? { username: pair, password: '' }
        : { username: pair.slice(0, idx).trim(), password: pair.slice(idx + 1) }
    })
    .filter((u) => u.username && u.password)

  const passwordEnabled = flag(env.AUTH_PASSWORD_ENABLED) && users.length > 0

  return {
    secret: env.AUTH_SECRET ?? '',
    publicUrl: env.AUTH_PUBLIC_URL?.replace(/\/$/, '') || null,
    allowedDomains: list(env.AUTH_ALLOWED_DOMAINS),
    allowedEmails: list(env.AUTH_ALLOWED_EMAILS),
    google: {
      enabled: googleEnabled,
      clientId: env.AUTH_GOOGLE_CLIENT_ID ?? '',
      clientSecret: env.AUTH_GOOGLE_CLIENT_SECRET ?? '',
      hd: env.AUTH_GOOGLE_HD?.trim() || null,
    },
    password: { enabled: passwordEnabled, users },
  }
}

/** The providers a user can actually pick right now (login screen renders these). */
export function enabledProviders(cfg = getAuthConfig()): ProviderMeta[] {
  const out: ProviderMeta[] = []
  if (cfg.google.enabled) out.push({ id: 'google', label: 'Continue with Google', kind: 'oauth' })
  if (cfg.password.enabled) out.push({ id: 'password', label: 'Username & password', kind: 'password' })
  return out
}

/** Central allow-list gate — applied to every provider's resolved identity. */
export function isEmailAllowed(email: string | null | undefined, cfg = getAuthConfig()): boolean {
  // No allow-list configured ⇒ anyone who authenticates is allowed.
  if (cfg.allowedDomains.length === 0 && cfg.allowedEmails.length === 0) return true
  if (!email) return false
  const e = email.toLowerCase()
  if (cfg.allowedEmails.includes(e)) return true
  const domain = e.split('@')[1] ?? ''
  return cfg.allowedDomains.includes(domain)
}
