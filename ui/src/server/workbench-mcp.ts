// The Workbench MCP — the governed surface agents drive their sandbox
// through. Registered in the MCP registry like any server (grantable per
// agent), dispatched in-process like app surfaces. Verbs are PROFILE-SCOPED:
// the dev workbench exposes the job lifecycle below; later profiles (design,
// data, content) expose their own gated verbs from the same dispatcher.
//
// The git-flow contract (why it never gets messy): Talaria cuts the branch
// (talaria/<ticket-ref>-<slug>) from default at start_job, the harness works
// ONLY inside that branch via the authenticated clone URL, finish_job opens
// the PR with the templated ticket-linked body. No raw pushes to default —
// the workbench token is the only credential in the sandbox, and every
// transition lands in task_activity.
import { db } from './db/pg'
import { branchAhead, cloneUrl, createBranch, createPullRequest, grantedRepos } from './github'
import { resolveWorkbench } from './workbench'
import { effortModel, effortModels, harness, type Effort } from './workbench-harnesses'

export interface WorkbenchJob {
  id: string
  agentId: string
  agentModel: string
  taskId: string | null
  repo: string
  branch: string
  effort: string
  plan: string
  status: 'started' | 'pr_open' | 'abandoned'
  prUrl: string | null
  summary: string
  createdAt: string
  updatedAt: string
}

const JOB_ROW = `id, agent_id as "agentId", agent_model as "agentModel", task_id as "taskId", repo, branch,
  effort, plan, status, pr_url as "prUrl", summary, created_at as "createdAt", updated_at as "updatedAt"`

const slugify = (v: string) =>
  v.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)

export const WORKBENCH_TOOLS = [
  {
    name: 'list_repos',
    description: 'The repositories YOUR workbench is granted. Work only these — anything else is out of bounds.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'start_job',
    description:
      'Start a workbench job for a ticket. Talaria cuts the working branch from the default branch and returns an authenticated clone URL. Work ONLY on that branch; commit and push to it as you go. For feature-scale work, write your plan first (it is recorded and rides into the PR). One job per ticket at a time.',
    inputSchema: {
      type: 'object',
      properties: {
        taskId: { type: 'string', description: 'The ticket this job implements' },
        repo: { type: 'string', description: 'owner/name — must be one of your granted repos' },
        effort: { type: 'string', enum: ['light', 'standard', 'heavy'], description: 'How hard this work is — routes tooling and review weight' },
        plan: { type: 'string', description: 'Your implementation plan: approach, files touched, test strategy. Required for standard/heavy.' },
      },
      required: ['repo'],
    },
  },
  {
    name: 'job_status',
    description: 'Your workbench jobs (optionally one by id): branch, status, PR link.',
    inputSchema: { type: 'object', properties: { jobId: { type: 'string' } } },
  },
  {
    name: 'finish_job',
    description:
      'Finish a job: Talaria verifies the branch has commits and opens the pull request with the ticket-linked body. Returns the PR URL — put it in your outcome report. Use abandon:true to close out a job that produced nothing.',
    inputSchema: {
      type: 'object',
      properties: {
        jobId: { type: 'string' },
        summary: { type: 'string', description: 'What the change does — becomes the PR body core' },
        abandon: { type: 'boolean' },
      },
      required: ['jobId'],
    },
  },
] as const

interface AgentCtx {
  id: string
  model: string
  department: string
  role: string | null
  workbench: 'off' | 'auto' | 'on'
  workbenchProfile: string | null
}

async function agentByModel(model: string): Promise<AgentCtx | null> {
  const sql = await db()
  const rows = (await sql`
    select id, model, department, role, workbench, workbench_profile as "workbenchProfile"
    from agent_defs where model = ${model} and enabled
  `) as unknown as AgentCtx[]
  return rows[0] ?? null
}

async function logTicket(taskId: string | null, actor: string, description: string): Promise<void> {
  if (!taskId) return
  const { logActivity } = await import('./tasks')
  await logActivity(taskId, actor, 'workbench', description).catch(() => {})
}

type ToolResult = { ok: true; value: unknown } | { ok: false; error: string }

async function callTool(agentModel: string, name: string, args: Record<string, unknown>): Promise<ToolResult> {
  const agent = await agentByModel(agentModel)
  if (!agent) return { ok: false, error: 'unknown agent' }
  const profile = await resolveWorkbench(agent)
  if (!profile) return { ok: false, error: 'no workbench attached — an admin can enable one on your agent settings' }
  const sql = await db()

  switch (name) {
    case 'list_repos':
      return {
        ok: true,
        value: {
          repos: await grantedRepos(agent.id),
          efforts: await effortModels(),
          note: 'Pick effort by the work, not the model: light = quick fixes, standard = regular features (plan required), heavy = hard cross-cutting work (plan required, used sparingly).',
        },
      }

    case 'start_job': {
      const repo = String(args.repo ?? '')
      const effort = ['light', 'standard', 'heavy'].includes(String(args.effort)) ? String(args.effort) : 'standard'
      const plan = String(args.plan ?? '').slice(0, 20_000)
      const taskId = typeof args.taskId === 'string' && args.taskId ? args.taskId : null
      if (!(await grantedRepos(agent.id)).includes(repo)) return { ok: false, error: `repo "${repo}" is not granted to you — list_repos shows yours` }
      if (effort !== 'light' && !plan.trim()) return { ok: false, error: 'standard/heavy work requires a plan first — describe approach, files, and test strategy in `plan`' }
      // One live job per ticket keeps branches 1:1 with work.
      if (taskId) {
        const dup = await sql`select 1 from workbench_jobs where task_id = ${taskId} and status = 'started' limit 1`
        if (dup.length) return { ok: false, error: 'a job is already running for this ticket — job_status shows it; finish or abandon it first' }
      }
      let ref = ''
      let title = ''
      if (taskId) {
        const rows = (await sql`select ticket_ref as "ticketRef", title from tasks where id = ${taskId}`) as unknown as Array<{ ticketRef: string | null; title: string }>
        ref = rows[0]?.ticketRef ?? ''
        title = rows[0]?.title ?? ''
      }
      const branch = `talaria/${ref ? ref.toLowerCase() : 'job'}-${slugify(title || String(args.repo)) || 'work'}`.slice(0, 80)
      const { base, created } = await createBranch(repo, branch).catch((e: Error) => {
        throw new Error(e.message)
      })
      const rows = (await sql`
        insert into workbench_jobs (agent_id, agent_model, task_id, repo, branch, effort, plan)
        values (${agent.id}, ${agent.model}, ${taskId}, ${repo}, ${branch}, ${effort}, ${plan})
        returning ${sql.unsafe(JOB_ROW)}
      `) as unknown as WorkbenchJob[]
      const job = rows[0]!
      await logTicket(taskId, agent.model, `workbench job started: ${repo} @ ${branch} (${effort})${plan ? ' — plan recorded' : ''}`)
      // Effort → model is Talaria's call: the agent picked the effort, the
      // platform resolves which model that means today. Invocation hints come
      // from the profile's harness adapters with the model slotted in.
      const model = await effortModel(effort as Effort)
      const harnesses = profile.harnesses
        .map((slug) => harness(slug))
        .filter((h): h is NonNullable<typeof h> => !!h)
        .map((h) => ({ harness: h.slug, run: model ? h.invoke.replace('<model>', model) : h.invoke }))
      return {
        ok: true,
        value: {
          jobId: job.id,
          repo,
          branch,
          base,
          resumed: !created,
          cloneUrl: await cloneUrl(repo),
          effort,
          model,
          harnesses,
          rules: `Clone with the URL above (token is short-lived — clone now). Work ONLY on ${branch}; commit and push to it as you go. Never touch ${base} directly. Use the ${effort}-effort model shown — escalate effort only when the work truly needs it. When done, call finish_job — Talaria opens the PR.`,
        },
      }
    }

    case 'job_status': {
      const jobId = typeof args.jobId === 'string' && args.jobId ? args.jobId : null
      const rows = jobId
        ? ((await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where id = $1 and agent_id = $2`, [jobId, agent.id])) as unknown as WorkbenchJob[])
        : ((await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where agent_id = $1 order by created_at desc limit 10`, [agent.id])) as unknown as WorkbenchJob[])
      return { ok: true, value: { jobs: rows.map(({ plan: _p, ...j }) => j) } }
    }

    case 'finish_job': {
      const jobId = String(args.jobId ?? '')
      const rows = (await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where id = $1 and agent_id = $2`, [jobId, agent.id])) as unknown as WorkbenchJob[]
      const job = rows[0]
      if (!job) return { ok: false, error: 'unknown job' }
      if (job.status !== 'started') return { ok: false, error: `job is already ${job.status}` }
      if (args.abandon === true) {
        await sql`update workbench_jobs set status = 'abandoned', updated_at = now() where id = ${job.id}`
        await logTicket(job.taskId, agent.model, `workbench job abandoned: ${job.repo} @ ${job.branch}`)
        return { ok: true, value: { status: 'abandoned' } }
      }
      const summary = String(args.summary ?? '').slice(0, 20_000)
      const { defaultBranch } = await import('./github')
      const base = await defaultBranch(job.repo)
      const ahead = await branchAhead(job.repo, base, job.branch)
      if (ahead === 0) return { ok: false, error: 'the branch has no commits yet — push your work first (or finish with abandon:true)' }
      let ticketLine = ''
      if (job.taskId) {
        const t = (await sql`select ticket_ref as "ticketRef", title from tasks where id = ${job.taskId}`) as unknown as Array<{ ticketRef: string | null; title: string }>
        if (t[0]) ticketLine = `Ticket: ${t[0].ticketRef ?? job.taskId} — ${t[0].title}\n\n`
      }
      const body =
        `${ticketLine}${summary || '(no summary provided)'}` +
        (job.plan ? `\n\n## Plan\n\n${job.plan}` : '') +
        `\n\n---\n🔧 Opened by ${agent.model} via the Talaria workbench (${job.effort} effort).`
      const t = (await sql`select ticket_ref as "ticketRef", title from tasks where id = ${job.taskId}`) as unknown as Array<{ ticketRef: string | null; title: string }>
      const title = t[0] ? `${t[0].ticketRef ? `[${t[0].ticketRef}] ` : ''}${t[0].title}`.slice(0, 100) : `Workbench: ${job.branch}`
      const pr = await createPullRequest(job.repo, { head: job.branch, base, title, body })
      await sql`update workbench_jobs set status = 'pr_open', pr_url = ${pr.url}, summary = ${summary}, updated_at = now() where id = ${job.id}`
      await logTicket(job.taskId, agent.model, `workbench PR opened: ${pr.url}`)
      return { ok: true, value: { prUrl: pr.url, prNumber: pr.number, note: 'Include this PR link in your outcome report.' } }
    }
  }
  return { ok: false, error: `unknown tool "${name}"` }
}

// ── JSON-RPC surface (same shape as the app dispatcher) ──────────────────────

interface Rpc {
  jsonrpc?: string
  id?: unknown
  method?: string
  params?: { name?: string; arguments?: Record<string, unknown>; [k: string]: unknown }
}

const result = (id: unknown, res: unknown) => ({ jsonrpc: '2.0', id: id ?? null, result: res })
const rpcError = (id: unknown, code: number, message: string) => ({ jsonrpc: '2.0', id: id ?? null, error: { code, message } })

export async function dispatchWorkbenchMcp(
  rpc: Rpc,
  agentModel: string,
  allowed: string[] | null,
): Promise<{ status: number; body: unknown | null }> {
  const tools = WORKBENCH_TOOLS.filter((t) => allowed === null || allowed.includes(t.name))
  switch (rpc.method) {
    case 'initialize':
      return {
        status: 200,
        body: result(rpc.id, {
          protocolVersion: (rpc.params?.protocolVersion as string) ?? '2025-03-26',
          capabilities: { tools: {} },
          serverInfo: { name: 'talaria-workbench', version: '1.0' },
        }),
      }
    case 'notifications/initialized':
      return { status: 202, body: null }
    case 'ping':
      return { status: 200, body: result(rpc.id, {}) }
    case 'tools/list':
      return { status: 200, body: result(rpc.id, { tools }) }
    case 'tools/call': {
      const tool = tools.find((t) => t.name === rpc.params?.name)
      if (!tool) return { status: 200, body: rpcError(rpc.id, -32602, `tool "${rpc.params?.name}" is not available here`) }
      try {
        const r = await callTool(agentModel, tool.name, rpc.params?.arguments ?? {})
        return {
          status: 200,
          body: result(rpc.id, {
            content: [{ type: 'text', text: r.ok ? JSON.stringify(r.value) : `Error: ${r.error}` }],
            isError: !r.ok,
          }),
        }
      } catch (e) {
        return { status: 200, body: result(rpc.id, { content: [{ type: 'text', text: `Error: ${(e as Error).message}` }], isError: true }) }
      }
    }
  }
  return { status: 200, body: rpcError(rpc.id, -32601, `method "${rpc.method}" not supported`) }
}
