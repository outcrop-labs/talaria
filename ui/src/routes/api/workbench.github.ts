import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { getGithubConfig, githubStatus, listInstallations, setGithubConfig } from '@/server/github'
import { logAudit } from '@/server/audit'

const Body = z.object({
  mode: z.enum(['app', 'pat']).nullable().optional(),
  pat: z.object({ token: z.string().max(400).nullable().optional() }).optional(),
  app: z
    .object({
      appId: z.string().max(40).optional(),
      installationId: z.string().max(40).optional(),
      privateKey: z.string().max(20_000).nullable().optional(),
    })
    .optional(),
})

// The Workbench's GitHub connection (admin): GET → live-verified redacted
// status (+ ?installations=1 lists where the App is installed, the easy-setup
// picker); PUT → patch config (secrets sealed); DELETE → disconnect.
export const Route = createFileRoute('/api/workbench/github')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        if (new URL(request.url).searchParams.get('installations')) {
          return json({ installations: await listInstallations() })
        }
        return json({ status: await githubStatus() })
      },
      PUT: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        await setGithubConfig(body)
        void logAudit({ actor: actorOf(user), action: 'workbench.github', targetType: 'workbench', targetId: 'github' })
        return json({ status: await githubStatus() })
      },
      DELETE: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const cur = await getGithubConfig()
        await setGithubConfig({ mode: null, pat: { token: null }, app: { appId: '', installationId: '', privateKey: null } })
        void logAudit({ actor: actorOf(user), action: 'workbench.github_disconnect', targetType: 'workbench', targetId: cur.mode ?? 'none' })
        return json({ ok: true })
      },
    },
  },
})
