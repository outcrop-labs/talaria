import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import {
  clearGoogleClientConfig,
  googleClientStatus,
  googleLoginEnabled,
  setGoogleClientConfig,
} from '@/server/google/client-config'
import { resolveOrigin } from '@/server/auth/google'
import { logAudit } from '@/server/audit'

const Body = z.object({
  clientId: z.string().min(1).max(200),
  /** Sealed server-side. Undefined = keep the stored secret (rotation without
   *  re-entering); '' / null = clear it. */
  clientSecret: z.string().max(400).nullable().optional(),
  /** Optional Workspace hosted-domain restriction. */
  hd: z.string().max(200).nullable().optional(),
})

// The Google OAuth client — the credential the whole Google integration (login
// + workspace connect) runs on. Admins register it here instead of editing
// ui/.env; the secret is SEALED and never read back. Deliberately requireAdmin:
// this is an org credential, not a grantable surface.
// GET → redacted status + the redirect URIs to register in Google Cloud Console
// PUT → save the client · DELETE → drop the Admin record (env fallback resumes)
export const Route = defineApi('/api/admin/google-client', {
  GET: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const origin = resolveOrigin(request)
    return json({
      status: await googleClientStatus(),
      loginEnabled: await googleLoginEnabled(),
      redirectUris: [
        { uri: `${origin}/api/integrations/google/callback`, what: 'your account connect (Settings)' },
        { uri: `${origin}/api/integrations/google/org/callback`, what: 'org connect (Admin)' },
        { uri: `${origin}/api/auth/google/callback`, what: 'Google login (only if you enable it)' },
      ],
    })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    await setGoogleClientConfig(body)
    void logAudit({
      actor: actorOf(user),
      action: 'google.client_config',
      targetType: 'google',
      targetId: 'client',
      after: { clientId: body.clientId, secretRotated: body.clientSecret !== undefined, hd: body.hd ?? undefined },
    })
    return json({ status: await googleClientStatus(), loginEnabled: await googleLoginEnabled() })
  },
  DELETE: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    await clearGoogleClientConfig()
    void logAudit({ actor: actorOf(user), action: 'google.client_config_clear', targetType: 'google', targetId: 'client' })
    return json({ status: await googleClientStatus(), loginEnabled: await googleLoginEnabled() })
  },
})
