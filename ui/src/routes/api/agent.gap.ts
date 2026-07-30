import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { reportGap } from '@/server/gaps'
import { getTask, logActivity } from '@/server/tasks'

const Body = z.object({
  kind: z.string().min(2).max(80),
  missing: z.string().min(5).max(300),
  needs: z.string().max(5000).optional(),
  taskId: z.string().uuid().optional(),
})

// POST — an agent reports a capability gap (the honesty loop). Deduped by
// work-shape server-side: repeats bump seen_count, never re-notify. Lands in
// the Studio's Suggested queue; the ticket (if given) gets an audit line.
export const Route = createFileRoute('/api/agent/gap')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const agent = agentName(request)
        if (!agent) return json({ error: 'x-agent-name required' }, { status: 400 })
        const body = await parseJson(request)
        const parsed = Body.safeParse(body)
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const task = parsed.data.taskId ? await getTask(parsed.data.taskId) : null
        const gap = await reportGap({
          agentModel: agent,
          kind: parsed.data.kind,
          missing: parsed.data.missing,
          needs: parsed.data.needs,
          boardId: task?.boardId ?? null,
          taskId: task?.id ?? null,
        })
        if (task) {
          await logActivity(
            task.id,
            agent,
            'gap',
            `reported a capability gap (${parsed.data.kind}${gap.seenCount > 1 ? `, seen ${gap.seenCount}×` : ''}): ${parsed.data.missing.slice(0, 200)}`,
          ).catch(() => {})
        }
        return json({
          ok: true,
          seenCount: gap.seenCount,
          note: gap.first
            ? 'Gap recorded — it will be suggested to the team in the Studio. Continue as best you can or set the ticket blocked with a comment.'
            : `Known gap (seen ${gap.seenCount}×) — already suggested to the team. Do not report it again; continue as best you can or set the ticket blocked.`,
        })
      },
    },
  },
})

async function parseJson(request: Request): Promise<unknown> {
  return request.json().catch(() => null)
}
