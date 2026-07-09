import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser, updateSessionUser } from '@/server/auth/session'
import { gatewayModels } from '@/server/llm-gateway'
import { memberModelAllowlist, modelAllowedFor } from '@/server/model-access'
import { getPreferredModel, setPreferredModel, setUserName } from '@/server/users'

// The signed-in user's profile. GET → preferences (preferred model). PUT
// { name?, preferredModel? } → update display name (users row + live session)
// and/or the model powering their AI drafting (null clears → server default).
export const Route = createFileRoute('/api/profile')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ preferredModel: await getPreferredModel(user.id) })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z
          .object({
            name: z.string().min(1).max(80).optional(),
            preferredModel: z.string().min(1).max(200).nullable().optional(),
          })
          .refine((b) => b.name !== undefined || b.preferredModel !== undefined, { message: 'nothing to update' })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        let updated = user
        if (parsed.data.name !== undefined) {
          const name = parsed.data.name.trim()
          await setUserName(user.id, name)
          updated = (await updateSessionUser(request, { name })) ?? user
        }
        if (parsed.data.preferredModel !== undefined) {
          // Members may only pick allowlisted models — enforced here, not just
          // hidden in the picker (admins gate the expensive brains).
          if (parsed.data.preferredModel !== null) {
            const allowed = modelAllowedFor(
              user.role,
              parsed.data.preferredModel,
              await memberModelAllowlist(),
              await gatewayModels(),
            )
            if (!allowed) return json({ error: 'that model is not available to you — ask an admin' }, { status: 403 })
          }
          await setPreferredModel(user.id, parsed.data.preferredModel)
        }
        return json({ user: updated })
      },
    },
  },
})
