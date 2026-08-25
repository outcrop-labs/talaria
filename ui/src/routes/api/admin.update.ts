import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import {
  applyUpdate,
  checkForUpdate,
  currentRev,
  reconcileUpdate,
  setAutoUpdate,
  updaterMode,
  updaterState,
  type RevInfo,
} from '@/server/updater'

// In-app updates (admin). GET reads the panel's world (mode, current commit,
// last check, last run, auto-update switch). POST runs an action: `check`
// fetches the remote and compares, `apply` starts the whole pull/build/
// restart sequence. PUT flips the auto-update switch, which is off until
// someone turns it on.
export const Route = defineApi('/api/admin/update', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    // The first read after a restart is what turns 'running' into 'done':
    // the new server is the only party that knows the update landed.
    await reconcileUpdate()
    const state = await updaterState()
    return json({
      mode: updaterMode(),
      current: await currentRev(),
      autoUpdate: state.autoUpdate,
      lastCheck: state.lastCheck,
      lastRun: state.lastRun,
      history: state.history,
    })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        action: z.enum(['check', 'apply']),
      }),
    )
    if (body instanceof Response) return body

    if (body.action === 'check') {
      let result: { behind: number; current: RevInfo; latest: RevInfo; branch: string } | null = null
      let error: string | null = null
      try {
        result = await checkForUpdate()
      } catch (e) {
        error = e instanceof Error ? e.message : String(e)
      }
      void logAudit({
        actor: actorOf(user),
        action: 'update.check',
        targetType: 'settings',
        after: result ? { behind: result.behind, latest: result.latest.rev } : { error },
      })
      if (error) return json({ error }, { status: 400 })
      return json(result)
    }

    // apply. The response leaves before the work does on success: the
    // process exits itself once the build+swap is done, and the panel
    // follows along by polling GET.
    const started = await applyUpdate('manual')
    void logAudit({
      actor: actorOf(user),
      action: 'update.apply',
      targetType: 'settings',
      after: { started: started.started, error: started.error ?? null },
    })
    if (!started.started) return json({ error: started.error ?? 'the update did not start' }, { status: 400 })
    return json({ started: true })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        autoUpdate: z.boolean(),
      }),
    )
    if (body instanceof Response) return body
    await setAutoUpdate(body.autoUpdate)
    void logAudit({
      actor: actorOf(user),
      action: 'settings.auto_update',
      targetType: 'settings',
      after: { autoUpdate: body.autoUpdate },
    })
    return json({ autoUpdate: body.autoUpdate })
  },
})
