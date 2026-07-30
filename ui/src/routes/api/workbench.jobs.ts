import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser, actorOf } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import { db } from '@/server/db/pg'
import { getTask, logActivity } from '@/server/tasks'
import { repoFlow } from '@/server/github'
import { mergeJobToTesting, type WorkbenchJob } from '@/server/workbench-mcp'

const JOB_ROW = `id, agent_model as "agentModel", task_id as "taskId", repo, branch, effort, plan, status,
  pr_url as "prUrl", summary, merged_testing_at as "mergedTestingAt", created_at as "createdAt", updated_at as "updatedAt"`

// Workbench jobs from the human side. GET ?taskId= → the ticket's jobs (board
// members — this is how the plan-approval gate and PR links surface on the
// ticket). PUT → approve / reject an awaiting job (board editors; rejection
// abandons with the reason in the ticket's audit trail).
export const Route = createFileRoute('/api/workbench/jobs')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const taskId = new URL(request.url).searchParams.get('taskId')
        if (!taskId) return json({ error: 'taskId required' }, { status: 400 })
        const task = await getTask(taskId)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        if (!(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        const sql = await db()
        const jobs = (await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where task_id = $1 order by created_at desc`, [taskId])) as unknown as Array<{ repo: string }>
        const withFlow = await Promise.all(jobs.map(async (j) => ({ ...j, testingBranch: (await repoFlow(j.repo)).testingBranch })))
        return json({ jobs: withFlow })
      },
      PUT: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ jobId: z.string().uuid(), action: z.enum(['approve', 'reject', 'merge_testing']), note: z.string().max(500).optional() }))
        if (body instanceof Response) return body
        const sql = await db()
        const [job] = (await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where id = $1`, [body.jobId])) as unknown as Array<{
          id: string
          taskId: string | null
          agentModel: string
          repo: string
          branch: string
          status: string
        }>
        if (!job) return json({ error: 'not found' }, { status: 404 })
        const task = job.taskId ? await getTask(job.taskId) : null
        if (!task || !canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        const actor = actorOf(user)
        if (body.action === 'merge_testing') {
          const r = await mergeJobToTesting(job as unknown as WorkbenchJob, actor).catch((e: Error) => ({ ok: false as const, error: e.message }))
          if (!r.ok) return json({ error: r.error }, { status: 400 })
          return json({ ok: true })
        }
        if (job.status !== 'awaiting_approval') return json({ error: `job is ${job.status}` }, { status: 400 })
        if (body.action === 'approve') {
          await sql`update workbench_jobs set status = 'started', updated_at = now() where id = ${job.id}`
          await logActivity(task.id, actor, 'workbench', `approved the workbench plan — ${job.agentModel} may build (${job.repo} @ ${job.branch})`)
        } else {
          await sql`update workbench_jobs set status = 'abandoned', updated_at = now() where id = ${job.id}`
          await logActivity(task.id, actor, 'workbench', `rejected the workbench plan${body.note ? `: ${body.note}` : ''} — job abandoned`)
        }
        return json({ ok: true })
      },
    },
  },
})
