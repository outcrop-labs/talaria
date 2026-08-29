import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { enabledProviders, getAuthConfig } from '@/server/auth/config'
import { googleLoginEnabled } from '@/server/google/client-config'
import type { ProviderMeta } from '@/server/auth/config'

// GET /api/auth/providers → the providers the login screen should render.
// Reflects exactly which providers are enabled AND fully configured right now.
// Google login follows the Admin UI toggle (AUTH_GOOGLE_ENABLED in env pins it
// on) and accepts credentials from the Admin UI record or the env —
// `enabledProviders` answers the env-only view, so the Google entry is
// recomputed against the merged client + login switch here.
export const Route = defineApi('/api/auth/providers', {
  GET: async () => {
    const cfg = getAuthConfig()
    const providers: ProviderMeta[] = []
    if (await googleLoginEnabled()) providers.push({ id: 'google', label: 'Continue with Google', kind: 'oauth' })
    providers.push(...enabledProviders(cfg).filter((p) => p.id !== 'google'))
    return json({
      providers,
      // Surfaced so the login screen can warn instead of silently failing.
      configured: Boolean(process.env.DATABASE_URL && process.env.REDIS_URL),
    })
  },
})
