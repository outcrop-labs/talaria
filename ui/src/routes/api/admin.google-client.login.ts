import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { googleLoginEnabled, setGoogleLoginEnabled } from '@/server/google/client-config'
import { logAudit } from '@/server/audit'

const Body = z.object({ enabled: z.boolean() })

// The Google LOGIN switch — the policy half of the client credential
// (PUT /api/admin/google-client stores the credential; this decides whether the
// login screen offers it). Flipping it is an admin's deliberate, audit-logged
// act; AUTH_GOOGLE_ENABLED pinned in env still wins towards on. A client must
// resolve for login to actually run, so the panel disables the toggle until one
// does.
export const Route = defineApi('/api/admin/google-client/login', {
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    await setGoogleLoginEnabled(body.enabled)
    void logAudit({
      actor: actorOf(user),
      action: 'google.login_config',
      targetType: 'google',
      targetId: 'login',
      after: { enabled: body.enabled },
    })
    return json({ loginEnabled: await googleLoginEnabled() })
  },
})
