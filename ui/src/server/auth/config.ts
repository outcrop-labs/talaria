// Auth configuration — resolved from the environment at request time.
//
//   AUTH_SECRET                signing key for session + state cookies (required)
//   AUTH_PUBLIC_URL            external origin (for OAuth redirect URIs) — optional,
//                              falls back to the request origin
//
//   AUTH_ALLOWED_DOMAINS       comma-separated email domains allowed GOOGLE sign-in
//   AUTH_ALLOWED_EMAILS        comma-separated exact emails allowed GOOGLE sign-in
//                              (password accounts are admitted by an admin at
//                              creation — these doors do not re-gate them)
//
//   Google OAuth — credentials from EITHER the Admin UI record (Admin → Org →
//   Google Workspace, stored in app_settings; the Rust client-config twin is
//   api/src/google/client.rs) OR:
//   AUTH_GOOGLE_CLIENT_ID / AUTH_GOOGLE_CLIENT_SECRET
//   AUTH_GOOGLE_ENABLED=1     login bootstrap — the Admin UI toggle beside the
//                             client is the primary switch; env pins login ON
//                             (see googleLoginEnabled)
//   AUTH_GOOGLE_HD             optional Google Workspace hosted-domain restriction
//
// Password accounts live in the database (user_password_credentials — Admin →
// People), not the environment. The first admin is minted by the first-run
// claim at /claim; see auth/claim.ts.

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
}

export function getAuthConfig(): AuthConfig {
  const env = process.env

  const googleEnabled =
    flag(env.AUTH_GOOGLE_ENABLED) && !!env.AUTH_GOOGLE_CLIENT_ID && !!env.AUTH_GOOGLE_CLIENT_SECRET

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
  }
}

/** The GOOGLE allow-list gate — applied to Google-resolved identities only.
 *  A password account was admitted by an admin (or the claim) when it was
 *  created; its login checks the stored hash and nothing else. */
export function isEmailAllowed(email: string | null | undefined, cfg = getAuthConfig()): boolean {
  // No allow-list configured ⇒ anyone who authenticates is allowed.
  if (cfg.allowedDomains.length === 0 && cfg.allowedEmails.length === 0) return true
  if (!email) return false
  const e = email.toLowerCase()
  if (cfg.allowedEmails.includes(e)) return true
  const domain = e.split('@')[1] ?? ''
  return cfg.allowedDomains.includes(domain)
}
