import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import {
  getNotifyDelivery,
  getNotifySettings,
  listNotifications,
  markNotificationsRead,
  setNotifyDelivery,
  setNotifySettings,
  unreadCount,
} from '@/server/notifications'
import { NOTIFY_CLASSES } from '@/lib/notifications'

// GET /api/notifications → the user's inbox, unread count, routing prefs, the
//   daily-digest switch, and whether this INSTANCE sends mail at all.
// PUT { ids? } → mark those read (or all when ids is omitted).
// PATCH { prefs?, digest? } → change where one or more event classes are
//   delivered, and/or turn the daily digest on or off.
// PATCH { delivery } → ADMIN ONLY. The instance-wide email master switch.
//
// Prefs ride along on the inbox read rather than getting a route of their own:
// they are seven short strings and a switch, the settings panel and the bell
// are the only readers, and both already hold this query.
// The class list is `NOTIFY_CLASSES`, not a second enum written out here —
// adding a class must not need an edit in two files to become settable.
//
// `digest` is a sibling of `prefs`, not a member of it: it is not an event
// class, it has no in-app reading, and putting it in the same record would have
// made the class check above accept a key that is not a class. A daily email
// with no off switch is how a sender gets filtered to spam, so it is settable
// here and nowhere else.
//
// `delivery` is the third thing here and the only ADMIN one: the instance-wide
// master switch, off until somebody turns it on. It rides on this route rather
// than getting an admin route of its own because the answer belongs in the same
// payload as the per-user prefs — a person whose mentions are set to "Both" and
// who is getting no mail deserves to be told that email is off for the whole
// instance, and the panel can only say so if it is told in the read it already
// makes. Reading it is not privileged (it describes what will happen to YOUR
// notifications); writing it is.
//
// `canSetDelivery` is on BOTH the GET and the PATCH response for the same
// reason the settings do: Settings → Notifications draws the control from it,
// and a panel that re-derives "am I an admin" from anywhere else is a panel
// that can disagree with the 403 this route would actually return. The switch
// it governs stops every notification and digest mail this instance sends —
// server/notifications.ts `sendGatedMail` is the single path all of it goes
// through, and scripts/check-invariants.mjs fails the build on a new one.
const ROUTE = z.enum(['in_app', 'email', 'both'])
const PrefsPatch = z
  .object({
    prefs: z
      .record(z.string().max(40), ROUTE)
      .refine((p) => Object.keys(p).length > 0, { message: 'nothing to update' })
      .refine((p) => Object.keys(p).every((k) => NOTIFY_CLASSES.some((c) => c.id === k)), {
        message: 'unknown notification class',
      })
      .optional(),
    digest: z.enum(['on', 'off']).optional(),
    delivery: z.object({ emailEnabled: z.boolean() }).optional(),
  })
  .refine((b) => b.prefs !== undefined || b.digest !== undefined || b.delivery !== undefined, {
    message: 'nothing to update',
  })

/** The instance switch, for a read that must not fail because of it. GET here
 *  answers "what is in your inbox"; if the one extra settings row cannot be
 *  read, the honest answer for a switch that defaults to off is off, and the
 *  inbox still renders. */
const deliveryOrOff = () => getNotifyDelivery().catch(() => ({ emailEnabled: false }))

export const Route = createFileRoute('/api/notifications')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({
          notifications: await listNotifications(user.id),
          unread: await unreadCount(user.id),
          ...(await getNotifySettings(user.id)),
          delivery: await deliveryOrOff(),
          /** Whether THIS user may flip the switch — the panel needs it to know
           *  whether to render a control or an explanation. */
          canSetDelivery: user.role === 'admin',
        })
      },
      PUT: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ ids: z.array(z.string().uuid()).max(200).optional() }))
        if (body instanceof Response) return body
        await markNotificationsRead(user.id, body.ids)
        return json({ ok: true })
      },
      PATCH: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, PrefsPatch)
        if (body instanceof Response) return body

        // The master switch decides whether the whole instance mails ANYBODY.
        // Checked here rather than by putting it on an admin route so that one
        // PATCH can never half-apply: a member who sends both gets 403 and
        // neither change, instead of their own prefs silently saved alongside a
        // rejected switch.
        if (body.delivery !== undefined) {
          if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
          const next = await setNotifyDelivery(body.delivery)
          // Audited: turning this on starts mailing every user in the
          // workspace, and "who did that, and when" is the first question.
          void logAudit({
            actor: actorOf(user),
            action: next.emailEnabled ? 'notifications.email.enabled' : 'notifications.email.disabled',
            targetType: 'notifications',
            after: next,
          })
        }

        const settings =
          body.prefs !== undefined || body.digest !== undefined
            ? await setNotifySettings(user.id, body)
            : await getNotifySettings(user.id)
        return json({ ...settings, delivery: await deliveryOrOff(), canSetDelivery: user.role === 'admin' })
      },
    },
  },
})
