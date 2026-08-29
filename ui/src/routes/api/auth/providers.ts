import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { instanceClaimable } from '@/server/auth/claim'
import { hasPasswordAccounts } from '@/server/auth/password-accounts'
import { googleLoginEnabled } from '@/server/google/client-config'
import type { ProviderMeta } from '@/server/auth/config'

// GET /api/auth/providers → the providers the login screen should render, and
// whether the instance is still UNCLAIMED. Everything is computed live:
//   • google — the Admin UI login toggle (or the AUTH_GOOGLE_ENABLED pin) AND
//     a resolvable client (Admin UI record or env);
//   • password — at least one DB-backed account exists (Admin → People);
//   • claimable — zero admins: the login screen offers /claim instead.
export const Route = defineApi('/api/auth/providers', {
  GET: async () => {
    const providers: ProviderMeta[] = []
    if (await googleLoginEnabled()) providers.push({ id: 'google', label: 'Continue with Google', kind: 'oauth' })
    if (await hasPasswordAccounts()) providers.push({ id: 'password', label: 'Username & password', kind: 'password' })
    return json({
      providers,
      claimable: await instanceClaimable(),
      // Surfaced so the login screen can warn instead of silently failing.
      configured: Boolean(process.env.DATABASE_URL && process.env.REDIS_URL),
    })
  },
})
