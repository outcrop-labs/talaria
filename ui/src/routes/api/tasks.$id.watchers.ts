import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser, type SessionUser } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import { addWatcher, getTask, listWatchers, removeWatcher } from '@/server/tasks'

const Body = z.object({ watcher: z.string().min(1).max(200) })

/** Is this watcher string the CALLER? Their email is what the Watch button
 *  sends (`user.email ?? user.name`), and the id forms are accepted because
 *  `user:<uuid>` is how a human is named everywhere else on a ticket. */
function isSelf(user: SessionUser, watcher: string): boolean {
  const w = watcher.trim().toLowerCase()
  if (user.email && w === user.email.trim().toLowerCase()) return true
  return w === user.id.toLowerCase() || w === `user:${user.id}`.toLowerCase()
}

// POST { watcher } → follow. DELETE { watcher } → unfollow.
//
// WHO MAY DO WHAT, and why each is the role it is:
//
//   POST   subscribing YOURSELF needs any board role — a viewer can read the
//          ticket, so a viewer may follow it. Subscribing SOMEONE ELSE writes
//          to another person's inbox and is an editor's call. The old guard was
//          `!task || !(await boardRole(...))`, truthiness over a
//          `'owner'|'editor'|'viewer'|null` return, so it never expressed
//          either of those — and the WATCHER itself was never checked against
//          the board at all. `addWatcher` now refuses a non-member (and says
//          so); this route decides only who may ask.
//
//   DELETE removing someone else is the editor's mirror of adding them.
//          REMOVING YOURSELF ALWAYS WORKS, with no board role of any kind.
//          That is the whole point: the person who most needs to unsubscribe is
//          the person getting mail about a board she cannot open, and the old
//          route answered her 403 — the worst shape this can take, because the
//          only remaining exit is asking someone else to stop it.
export const Route = createFileRoute('/api/tasks/$id/watchers')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        const role = await boardRole(user.id, task.boardId)
        if (role === null) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        if (!isSelf(user, body.watcher) && !canEdit(role)) {
          return json({ error: 'only a board editor can make someone else follow this ticket' }, { status: 403 })
        }
        const added = await addWatcher(params.id, body.watcher)
        if (!added.ok) return json({ error: added.error }, { status: 400 })
        return json({ watchers: await listWatchers(params.id) })
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        const role = await boardRole(user.id, task.boardId)
        if (!isSelf(user, body.watcher) && !canEdit(role)) return json({ error: 'forbidden' }, { status: 403 })
        await removeWatcher(params.id, body.watcher)
        // The unsubscribe hatch must not become a disclosure hatch: someone with
        // no membership gets confirmation that they are off the ticket, never
        // the list of who else is on it.
        if (role === null) return json({ unwatched: true })
        return json({ watchers: await listWatchers(params.id) })
      },
    },
  },
})
