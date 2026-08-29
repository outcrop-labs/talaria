import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import {
  createPasswordAccount,
  listPasswordAccounts,
  removePasswordAccount,
  setPasswordAccountPassword,
} from '@/server/auth/password-accounts'
import { logAudit } from '@/server/audit'

const Create = z.object({
  email: z.preprocess(
    (v) => (typeof v === 'string' ? v.trim().toLowerCase() : v),
    z.string().email().max(200),
  ),
  password: z.string().min(8).max(1000),
  name: z.string().max(200).optional(),
})
const Set = z.object({ userId: Uuid, password: z.string().min(8).max(1000) })
const Remove = z.object({ userId: Uuid })

// Admin console API for DB-backed password accounts (Admin → People).
// GET → the account list. POST → create an account. PUT → set/reset a
// password. DELETE → remove the account (the person stays). Admins only.
// Audit entries carry the email, never the password or its hash.
export const Route = defineApi('/api/admin/password-accounts', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    return json({ accounts: await listPasswordAccounts() })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Create)
    if (body instanceof Response) return body
    const result = await createPasswordAccount(body)
    if (!result.ok) {
      if (result.reason === 'email-taken') {
        return json({ error: 'An account with that email already exists' }, { status: 409 })
      }
      return json({ error: 'Could not create the account' }, { status: 400 })
    }
    void logAudit({
      actor: actorOf(user),
      action: 'user.password_add',
      targetType: 'user',
      targetId: result.userId,
      targetLabel: body.email,
      after: { email: body.email },
    })
    return json({ ok: true, userId: result.userId })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Set)
    if (body instanceof Response) return body
    const result = await setPasswordAccountPassword(body.userId, body.password)
    if (!result.ok) {
      const error =
        result.reason === 'email-taken'
          ? 'That email already belongs to another password account'
          : result.reason === 'no-email'
            ? 'That user has no email to hang a password account from'
            : 'No such user'
      return json({ error }, { status: result.reason === 'not-found' ? 404 : result.reason === 'email-taken' ? 409 : 400 })
    }
    void logAudit({
      actor: actorOf(user),
      action: 'user.password_set',
      targetType: 'user',
      targetId: body.userId,
      targetLabel: result.email,
      after: { email: result.email },
    })
    return json({ ok: true })
  },
  DELETE: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Remove)
    if (body instanceof Response) return body
    const email = await removePasswordAccount(body.userId)
    if (email === null) return json({ error: 'No password account for that user' }, { status: 404 })
    void logAudit({
      actor: actorOf(user),
      action: 'user.password_remove',
      targetType: 'user',
      targetId: body.userId,
      targetLabel: email,
      after: { email },
    })
    return json({ ok: true })
  },
})
