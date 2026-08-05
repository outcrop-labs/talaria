import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requirePerm } from '@/server/api-guard'
import { listReachableRepos, listRepoFlows, setRepoFlow } from '@/server/github'

// Per-repo git flow (PR base + optional testing branch). GET → configured
// flows + the reachable pool; PUT → set one repo's flow. agents.manage.
export const Route = defineApi('/api/workbench/flow', {
  GET: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const [flows, repos] = await Promise.all([listRepoFlows(), listReachableRepos()])
    return json({ flows, repos })
  },
  PUT: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        repo: z.string().min(3).max(200),
        baseBranch: z.string().max(100).nullable().optional(),
        testingBranch: z.string().max(100).nullable().optional(),
      }),
    )
    if (body instanceof Response) return body
    await setRepoFlow(body.repo, { baseBranch: body.baseBranch?.trim() || null, testingBranch: body.testingBranch?.trim() || null })
    return json({ flows: await listRepoFlows() })
  },
})
