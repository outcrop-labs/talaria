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
//   Google OAuth is not read here: the client (the Admin UI record, else
//   AUTH_GOOGLE_*) is resolved by the Rust api, talaria-google-client.
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
}

export function getAuthConfig(): AuthConfig {
  const env = process.env
  return {
    secret: env.AUTH_SECRET ?? '',
    publicUrl: env.AUTH_PUBLIC_URL?.replace(/\/$/, '') || null,
    allowedDomains: list(env.AUTH_ALLOWED_DOMAINS),
    allowedEmails: list(env.AUTH_ALLOWED_EMAILS),
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
