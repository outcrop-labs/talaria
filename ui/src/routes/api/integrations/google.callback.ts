import { defineApi } from '@/server/api-route'
import { completeGoogleConnect, googleConnectRedirectUri, handleConnectCallback } from '@/server/google/oauth'

// GET /api/integrations/google/callback → verify state, exchange the code for an
// offline refresh token, and store the connection for the signed-in user.
// The flow itself (gate, state, exchange, bounce-back) is the shared connect
// body; what THIS route adds is the meaning — the tokens are THIS user's, and
// the human lands on the integrations tab whose flash reads the status param
// (the CONNECTIONS tab specifically: that flash is mounted by
// IntegrationsSection, which only exists there).
export const Route = defineApi('/api/integrations/google/callback', {
  GET: async ({ request }) =>
    handleConnectCallback(request, {
      complete: completeGoogleConnect,
      redirectUri: googleConnectRedirectUri(request),
      landing: (status) => `/settings/connections?google=${encodeURIComponent(status)}`,
      logTag: 'integrations/google',
    }),
})
