import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { updateSessionUser } from '@/server/auth/session'
import { gatewayModels } from '@/server/llm-gateway'
import { memberModelAllowlist, modelAllowedFor } from '@/server/model-access'
import { getPreferredEffort, getPreferredModel, getTimezone, setPreferredEffort, setPreferredModel, setTimezone, setUserName } from '@/server/users'
import { isValidTimeZone } from '@/lib/timezone'

// The signed-in user's profile. GET → preferences (preferred model, preferred
// effort, timezone). PUT { name?, preferredModel?, preferredEffort?,
// timezone? } → update display name (users row + live session), the model
// powering their AI drafting (null clears → server default), their
// platform-default reasoning effort (null clears → every model's own
// default), and/or their IANA zone (null clears → follow the workspace
// zone).
export const Route = defineApi('/api/me', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({
      preferredModel: await getPreferredModel(user.id),
      preferredEffort: await getPreferredEffort(user.id),
      timezone: await getTimezone(user.id),
    })
  },
  PUT: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z
        .object({
          name: z.string().min(1).max(80).optional(),
          preferredModel: z.string().min(1).max(200).nullable().optional(),
          preferredEffort: z.string().min(1).max(24).nullable().optional(),
          timezone: z.string().min(1).max(64).nullable().optional(),
        })
        .refine(
          (b) =>
            b.name !== undefined || b.preferredModel !== undefined || b.preferredEffort !== undefined || b.timezone !== undefined,
          { message: 'nothing to update' },
        ),
    )
    if (body instanceof Response) return body
    let updated = user
    if (body.name !== undefined) {
      const name = body.name.trim()
      await setUserName(user.id, name)
      updated = (await updateSessionUser(request, { name })) ?? user
    }
    if (body.preferredModel !== undefined) {
      // Members may only pick allowlisted models — enforced here, not just
      // hidden in the picker (admins gate the expensive brains).
      if (body.preferredModel !== null) {
        const allowed = modelAllowedFor(
          user.role,
          body.preferredModel,
          await memberModelAllowlist(),
          await gatewayModels(),
        )
        if (!allowed) return json({ error: 'that model is not available to you — ask an admin' }, { status: 403 })
      }
      await setPreferredModel(user.id, body.preferredModel)
    }
    if (body.preferredEffort !== undefined) {
      // Deliberately NOT validated against any one model's published levels:
      // the preference travels across every model the user talks to (their
      // preferred model, agent personas, tiers), and each surface applies it
      // only where that model's metadata vouches for the level — the Settings
      // control only ever offers the current model's real levels, and a stale
      // or foreign level is inert everywhere else. A length bound is the whole
      // server-side contract.
      await setPreferredEffort(user.id, body.preferredEffort)
    }
    if (body.timezone !== undefined) {
      // An IANA name this runtime can resolve, or a refusal — the stored
      // value drives scheduled work (brief opens, digest sends), so a typo
      // must die here rather than warn from localMoment at 6am. Null passes
      // straight through: it IS the setting "follow the workspace zone".
      if (body.timezone !== null) {
        const tz = body.timezone.trim()
        if (!isValidTimeZone(tz)) return json({ error: 'not a recognized time zone' }, { status: 400 })
        await setTimezone(user.id, tz)
      } else {
        await setTimezone(user.id, null)
      }
    }
    return json({ user: updated })
  },
})
