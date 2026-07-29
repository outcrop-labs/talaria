import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { HANDLE_RE, createPersonalAgent, personalAgentFor, updatePersonalAgent } from '@/server/personal-agent'

const Name = z.string().trim().min(1, 'give it a name').max(60, 'keep the name under 60 characters')
const Handle = z
  .string()
  .trim()
  .regex(HANDLE_RE, 'handles are 2–30 lowercase letters/numbers, starting with a letter')

const CreateBody = z.object({
  name: Name.optional(),
  handle: Handle.optional(),
  personality: z.string().max(4000).optional(),
})

const PatchBody = z
  .object({
    name: Name.optional(),
    handle: Handle.optional(),
    personality: z.string().max(4000).optional(),
    /** A tier name from the assistant's `tiers` — becomes the default model. */
    model: z.string().trim().min(1).max(60).optional(),
  })
  .refine((b) => Object.values(b).some((v) => v !== undefined), { message: 'nothing to update' })

/** "agent \"x\" already exists" (slug collision) → something a person can act on. */
const friendly = (msg: string) => (/already exists/.test(msg) ? 'that handle is taken — pick another' : msg)

// The signed-in user's personal assistant. GET → theirs (or null), with
// personality + live status. POST → create + start one, optionally named/
// personalized (idempotent: returns the existing one, re-enabling if retired).
// PATCH → owner-scoped rename / personality edit. Any signed-in user; every
// operation is keyed on agent_defs.owner_user_id, never on a client-sent id.
export const Route = createFileRoute('/api/me/assistant')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({ assistant: await personalAgentFor(user.id) })
      },
      POST: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, CreateBody)
        if (body instanceof Response) return body
        try {
          const assistant = await createPersonalAgent({ id: user.id, email: user.email, name: user.name }, body)
          return json({ assistant })
        } catch (e) {
          return json({ error: friendly((e as Error).message) }, { status: 400 })
        }
      },
      PATCH: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, PatchBody)
        if (body instanceof Response) return body
        try {
          const assistant = await updatePersonalAgent({ id: user.id, email: user.email, name: user.name }, body)
          return json({ assistant })
        } catch (e) {
          return json({ error: friendly((e as Error).message) }, { status: 400 })
        }
      },
    },
  },
})
