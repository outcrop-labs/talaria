import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { enabledProviders, getAuthConfig } from '@/server/auth/config'

// GET /api/auth/providers → the providers the login screen should render.
// Reflects exactly which providers are enabled AND fully configured right now.
export const Route = defineApi('/api/auth/providers', {
  GET: async () => {
    const cfg = getAuthConfig()
    return json({
      providers: enabledProviders(cfg),
      // Surfaced so the login screen can warn instead of silently failing.
      configured: Boolean(process.env.DATABASE_URL && process.env.REDIS_URL),
    })
  },
})
