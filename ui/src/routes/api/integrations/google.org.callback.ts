import { defineApi } from '@/server/api-route'
import { completeGoogleOrgConnect, googleOrgConnectRedirectUri, handleConnectCallback } from '@/server/google/oauth'

// GET /api/integrations/google/org/callback → store the SHARED org connection.
// The shared connect body plus the org's two differences: only an admin may
// tie the org's containers to a Google account, and the landing page is the
// admin panel whose googleOrg flash reads the status param.
export const Route = defineApi('/api/integrations/google/org/callback', {
  GET: async ({ request }) =>
    handleConnectCallback(request, {
      complete: (userId, code, redirectUri, nowMs) => completeGoogleOrgConnect(userId, code, redirectUri, nowMs),
      redirectUri: googleOrgConnectRedirectUri(request),
      landing: (status) => `/admin?googleOrg=${encodeURIComponent(status)}`,
      logTag: 'integrations/google/org',
      adminOnly: true,
    }),
})
