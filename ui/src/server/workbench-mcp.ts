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
import { subjectModel, type AgentSubject } from './agent-auth'
import { db } from './db/pg'
import { branchAhead, cloneUrl, createBranch, createPullRequest, effectiveBase, grantedRepos, mergeInto, repoFlow } from './github'
import { resolveWorkbench } from './workbench'
import { effortModel, effortModels, harnessModelArg, listHarnessDefs, type Effort } from './workbench-harnesses'
import { getGithubConfig, githubStatus } from './github'

export interface WorkbenchJob {
  id: string
  agentId: string
  agentModel: string
  taskId: string | null
  repo: string
  branch: string
  effort: Effort
  plan: string
  status: 'awaiting_approval' | 'started' | 'pr_open' | 'abandoned'
  prUrl: string | null
  summary: string
  createdAt: string
  updatedAt: string
}

const JOB_ROW = `id, agent_id as "agentId", agent_model as "agentModel", task_id as "taskId", repo, branch,
  effort, plan, status, pr_url as "prUrl", summary, created_at as "createdAt", updated_at as "updatedAt"`

const slugify = (v: string) =>
  v.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)

/** Ticket ref + title (refs are computed: board prefix + ticket_no). */
async function ticketRefOf(taskId: string): Promise<{ ref: string; title: string } | null> {
  const sql = await db()
  const rows = (await sql`
    select case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end as "ticketRef", t.title
    from tasks t join boards b on b.id = t.board_id where t.id = ${taskId}
  `) as unknown as Array<{ ticketRef: string | null; title: string }>
  return rows[0] ? { ref: rows[0].ticketRef ?? '', title: rows[0].title } : null
}

export const WORKBENCH_TOOLS = [
  {
    name: 'doctor',
    description:
      'Diagnose YOUR workbench end to end: profile, chosen harness (with a probe command to run in your shell), auth, GitHub connection, repo grants, effort→model map, and pass-through config locations. Run this first when anything about your workbench misbehaves — or before your first job.',
    inputSchema: { type: 'object', properties: {} },
  },
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
        taskId: { type: 'string', description: 'The ticket this job implements — ALWAYS pass it when the work came from a ticket; it links the branch, audit trail, plan gate, and PR to the ticket.' },
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
    name: 'merge_to_testing',
    description:
      "Merge a job's branch into the repo's TESTING branch for integration testing (only when the repo has one configured). The PR to the base branch stays open and unmerged — testing is a sideline, never the way work ships.",
    inputSchema: { type: 'object', properties: { jobId: { type: 'string' } }, required: ['jobId'] },
  },
  {
    name: 'request_repo',
    description:
      'Request a NEW repository in an approved org — a human approves before anything is created (you will see it in list_repos once granted). Use only when the work genuinely needs a fresh repo; explain why.',
    inputSchema: {
      type: 'object',
      properties: {
        org: { type: 'string', description: 'The GitHub org — must be on the approved list (doctor shows it)' },
        name: { type: 'string', description: 'Repo name (lowercase, dashes)' },
        description: { type: 'string', description: 'One-line repo description' },
        why: { type: 'string', description: 'Why this work needs a new repo' },
        taskId: { type: 'string', description: 'The ticket that motivated it' },
      },
      required: ['org', 'name', 'why'],
    },
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
  displayName: string
  department: string
  role: string | null
  workbench: 'off' | 'auto' | 'on'
  workbenchProfile: string | null
  workbenchHarness: string | null
  workbenchModels: Partial<Record<Effort, string>>
}

async function agentByModel(model: string): Promise<AgentCtx | null> {
  const sql = await db()
  const rows = (await sql`
    select id, model, display_name as "displayName", department, role, workbench, workbench_profile as "workbenchProfile",
           workbench_harness as "workbenchHarness", workbench_models as "workbenchModels"
    from agent_defs where model = ${model} and enabled
  `) as unknown as AgentCtx[]
  return rows[0] ?? null
}

async function logTicket(taskId: string | null, actor: string, description: string): Promise<void> {
  if (!taskId) return
  const { logActivity } = await import('./tasks')
  await logActivity(taskId, actor, 'workbench', description).catch(() => {})
}

/** THE GATE for a caller-supplied taskId. Verbs here take the ticket from the
 *  agent, and everything downstream either DISCLOSES it (the ticket ref and
 *  title ride into the branch name and, at finish_job, into a public PR title
 *  and body) or WRITES to it (a plan comment authored as the agent, the plan
 *  artifact chip on the ticket, workbench audit lines). None of that reaches
 *  `updateTask`, so the two rules it carries are enforced here instead:
 *    · the board's agent policy must allow this agent (a ticket on a board the
 *      agent cannot see is not its work — and its title is not its business)
 *    · a ticket a person has taken off the table takes no further agent writes
 *  The second rule is `closedToAgents`, IMPORTED. It used to be a local copy
 *  carrying only the closed-status third of it, so an agent could start a
 *  workbench job against an archived ticket — plan comment, artifact chip and
 *  all. Unknown and not-allowed refuse with the SAME message: a distinct "no
 *  such ticket" would turn this verb into a ticket enumeration oracle. */
async function authorizeTicket(subject: AgentSubject, taskId: string): Promise<{ ok: true } | { ok: false; error: string }> {
  const { getTask, closedToAgents } = await import('./tasks')
  const task = await getTask(taskId).catch(() => null)
  const deny = {
    ok: false as const,
    error: `taskId "${taskId}" is not a ticket you may work — it does not exist, or its board does not allow you. Omit taskId, or ask an admin for access to that board.`,
  }
  if (!task) return deny
  const { boardAllowsAgent } = await import('./boards')
  // The subject carries proof when the caller threaded it through; a bare model
  // string is read as proven (a legacy caller can never present a privileged
  // name), so this is honest either way.
  if (!(await boardAllowsAgent(task.boardId, subject))) return deny
  const shut = await closedToAgents(task)
  if (shut) {
    return {
      ok: false,
      error: `${shut}. Ticket ${task.ticketRef ?? taskId} is "${task.status}". Ask for it to be reopened, or work the follow-up ticket.`,
    }
  }
  return { ok: true }
}

/** Board policy alone, for the DISCLOSURE point (finish_job): may this agent
 *  still see this ticket? Deliberately NOT the closed check — a person closing
 *  the ticket while the job ran should not cost the PR its ticket link. */
async function ticketStillOurs(agentModel: string, taskId: string): Promise<boolean> {
  const { getTask } = await import('./tasks')
  const task = await getTask(taskId).catch(() => null)
  if (!task) return false
  const { boardAllowsAgent } = await import('./boards')
  return boardAllowsAgent(task.boardId, agentModel)
}

type ToolResult = { ok: true; value: unknown } | { ok: false; error: string }

async function callTool(subject: AgentSubject, name: string, args: Record<string, unknown>): Promise<ToolResult> {
  const agentModel = subjectModel(subject)
  const agent = await agentByModel(agentModel)
  if (!agent) return { ok: false, error: 'unknown agent' }
  const profile = await resolveWorkbench(agent)
  if (!profile) return { ok: false, error: 'no workbench attached — an admin can enable one on your agent settings' }
  const sql = await db()

  switch (name) {
    case 'doctor': {
      const registry = await listHarnessDefs()
      const chosenSlug = agent.workbenchHarness && profile.harnesses.includes(agent.workbenchHarness) ? agent.workbenchHarness : profile.harnesses[0]
      const h = registry.find((x) => x.slug === chosenSlug)
      const gh = await githubStatus().catch(() => null)
      const repos = await grantedRepos(agent.id)
      const checks: string[] = []
      checks.push(`profile: ${profile.name} (${profile.slug}) — attached`)
      checks.push(h ? `harness: ${h.label} (${h.slug}, ${h.source}) — chosen` : `harness: "${chosenSlug}" NOT in the registry — pick another or ask an admin`)
      if (h) checks.push(h.auth === 'gateway' ? 'auth: Talaria gateway (no key needed on your side)' : `auth: native ${(h.auth as { provider: string }).provider} key expected in ${(h.auth as { envVar: string }).envVar}`)
      checks.push(gh?.configured ? `github: connected${gh.account ? ` as ${gh.account}` : ''}` : 'github: NOT connected — jobs cannot start (an admin connects it in Admin → Org)')
      checks.push(repos.length ? `repos: ${repos.join(', ')}` : 'repos: none granted — ask an admin to grant repos on your agent settings')
      return {
        ok: true,
        value: {
          checks,
          harness: h
            ? {
                slug: h.slug,
                guide: h.guide,
                probe: h.probe ?? null,
                mcpTools: h.mcpServe ? 'registered on your config when this harness is chosen' : 'none — drive it via jsonRun',
                passthroughConfig: h.mcpConfig ? `/opt/workbench-config/${h.mcpConfig.filename}` : null,
              }
            : null,
          efforts: await effortModels(agent.workbenchModels),
          workspaceRoot: '/opt/data/workbench/jobs/<jobId>',
          sessionHistory: '/opt/data/workbench/harness (persistent, shared with your department)',
          next: h?.probe
            ? `Run the probe in your shell to verify the harness binary: ${h.probe}`
            : 'No probe declared — try the harness directly on your first job.',
        },
      }
    }

    case 'list_repos':
      return {
        ok: true,
        value: {
          repos: await grantedRepos(agent.id),
          efforts: await effortModels(agent.workbenchModels),
          note: 'Pick effort by the work, not the model: light = quick fixes, standard = regular features (plan required), heavy = hard cross-cutting work (plan required, used sparingly).',
        },
      }

    case 'start_job': {
      const repo = String(args.repo ?? '')
      const effort: Effort = args.effort === 'light' || args.effort === 'heavy' ? args.effort : 'standard'
      const plan = String(args.plan ?? '').slice(0, 20_000)
      const taskId = typeof args.taskId === 'string' && args.taskId ? args.taskId : null
      if (!(await grantedRepos(agent.id)).includes(repo)) return { ok: false, error: `repo "${repo}" is not granted to you — list_repos shows yours` }
      if (effort !== 'light' && !plan.trim()) return { ok: false, error: 'standard/heavy work requires a plan first — describe approach, files, and test strategy in `plan`' }
      // BEFORE anything reads the ticket: the caller supplied this id, so board
      // policy and the closed-ticket rule decide whether it may be touched at
      // all. Everything below (ref + title in the branch name, the plan comment
      // and artifact chip on the ticket, the audit line, and finish_job's public
      // PR title) depends on this having passed.
      if (taskId) {
        const auth = await authorizeTicket(subject, taskId)
        if (!auth.ok) return auth
      }
      // One live job per ticket keeps branches 1:1 with work.
      if (taskId) {
        const dup = await sql`select 1 from workbench_jobs where task_id = ${taskId} and status = 'started' limit 1`
        if (dup.length) return { ok: false, error: 'a job is already running for this ticket — job_status shows it; finish or abandon it first' }
      }
      const tRef = taskId ? await ticketRefOf(taskId) : null
      const ref = tRef?.ref ?? ''
      const title = tRef?.title ?? ''
      const branch = ref
        ? `talaria/${ref.toLowerCase()}-${slugify(title) || 'work'}`.slice(0, 80)
        : `talaria/job-${slugify(String(args.repo))}-${Math.random().toString(36).slice(2, 8)}`.slice(0, 80)
      const { base, created } = await createBranch(repo, branch)
      // Plan-first, gated by effort: light auto-proceeds; standard proceeds
      // with the plan posted to the ticket (audit trail); heavy WAITS for a
      // human to approve the plan from the ticket before any clone URL exists.
      const gated = effort === 'heavy' && !!taskId
      const rows = (await sql`
        insert into workbench_jobs (agent_id, agent_model, task_id, repo, branch, effort, plan, status)
        values (${agent.id}, ${agent.model}, ${taskId}, ${repo}, ${branch}, ${effort}, ${plan}, ${gated ? 'awaiting_approval' : 'started'})
        returning ${sql.unsafe(JOB_ROW)}
      `) as unknown as WorkbenchJob[]
      const job = rows[0]!
      if (taskId && plan.trim()) {
        await sql`
          insert into task_comments (task_id, author, content)
          values (${taskId}, ${agent.model}, ${`**Workbench plan** (${effort} effort · ${repo}):\n\n${plan}`})
        `.catch(() => {})
        // The plan also becomes a markdown ARTIFACT attached to the ticket —
        // durable and versioned, not just scrollback. Filed under the agent's
        // Plans cabinet; org-visible like the ticket it belongs to.
        void (async () => {
          const { agentCategoryFolder, createArtifact, saveArtifact } = await import('./artifacts')
          const { describeAgent } = await import('./gateway')
          const label = agent.displayName || describeAgent(agent.model).label
          const artifact = await createArtifact({
            kind: 'doc',
            title: `Plan — ${ref || title || repo}`.slice(0, 120),
            createdBy: agent.model,
            ownerUserId: null,
            folderId: await agentCategoryFolder(label, 'Plans', agent.model).catch(() => null),
          })
          await saveArtifact(artifact.id, { body: `# Workbench plan — ${ref ? `${ref} · ` : ''}${title || repo}\n\n_${effort} effort · ${repo} · by ${label}_\n\n${plan}` }, agent.model)
          const chip = { id: artifact.id, filename: artifact.title || 'Plan', mime: 'ref/artifact', size: 0, refType: 'artifact' }
          // Through `updateTask` as the AGENT, never `update tasks set …`: raw
          // SQL here was the one agent-reachable write to `tasks` that skipped
          // the human-in-the-loop invariant entirely (it would have re-stamped
          // a ticket a person closed mid-job). The agent actor also gets the
          // attachment activity line and the board push for free.
          const { getTask, updateTask } = await import('./tasks')
          const cur = await getTask(taskId)
          const have = Array.isArray(cur?.attachments) ? cur.attachments : []
          if (!have.some((a) => (a as { id?: string }).id === artifact.id)) {
            await updateTask(taskId, { attachments: [...have, chip] }, { kind: 'agent', id: agent.model })
          }
        })().catch(() => {})
      }
      await logTicket(
        taskId,
        agent.model,
        gated
          ? `workbench job awaiting plan approval: ${repo} @ ${branch} (heavy)`
          : `workbench job started: ${repo} @ ${branch} (${effort})${plan ? ' — plan recorded' : ''}`,
      )
      // Effort → model is Talaria's call: the agent picked the effort, the
      // platform resolves which model that means today. Invocation hints come
      // from the profile's harness adapters with the model slotted in.
      const model = await effortModel(effort, agent.workbenchModels)
      // The agent's chosen harness leads (falling back to the profile's
      // first); its invocation line carries the model in the harness's own
      // syntax, so it's directly runnable.
      const chosenSlug = agent.workbenchHarness && profile.harnesses.includes(agent.workbenchHarness) ? agent.workbenchHarness : profile.harnesses[0]
      const registry = await listHarnessDefs()
      const harnesses = profile.harnesses
        .map((slug) => registry.find((h) => h.slug === slug))
        .filter((h): h is NonNullable<typeof h> => !!h)
        .sort((a, b) => (a.slug === chosenSlug ? -1 : b.slug === chosenSlug ? 1 : 0))
        .map((h) => ({
          harness: h.slug,
          chosen: h.slug === chosenSlug,
          run: model ? h.invoke.replace('<model>', harnessModelArg(h, model)) : h.invoke,
          ...(h.jsonInvoke ? { jsonRun: model ? h.jsonInvoke.replace('<model>', harnessModelArg(h, model)) : h.jsonInvoke } : {}),
          ...(h.slug === chosenSlug ? { guide: h.guide } : {}),
        }))
      // One WORKSPACE per job — concurrent jobs never collide — under the
      // persistent volume, so clones and harness sessions survive restarts.
      const workdir = `/opt/data/workbench/jobs/${job.id}`
      return {
        ok: true,
        value: {
          jobId: job.id,
          repo,
          branch,
          base,
          resumed: !created,
          status: job.status,
          workdir,
          ...(gated ? {} : { cloneUrl: await cloneUrl(repo) }),
          effort,
          model,
          harnesses,
          rules: gated
            ? `Heavy work waits for a human: your plan is on the ticket for approval. Poll job_status — once approved it returns the clone URL. Do NOT begin building.`
            : `Clone with the URL above INTO your workdir (mkdir -p ${workdir} first; the token is short-lived — clone now). One workspace per job: never work outside it, so concurrent jobs stay isolated. Your harness's session history persists under /opt/data/workbench/harness and is shared with your department — resume prior sessions or pick up a teammate's hand-off from there. Work ONLY on ${branch}; commit and push to it as you go — your sandbox is preconfigured so commits are authored as YOU (do not override git identity). Never touch ${base} directly. Use your CHOSEN harness (first in the list, marked chosen) with the ${effort}-effort model shown — via its MCP tools if registered on your config, else its jsonRun form; read structured results, never scrape raw logs. Escalate effort only when the work truly needs it. When done, call finish_job — Talaria opens the PR.`,
        },
      }
    }

    case 'job_status': {
      const jobId = typeof args.jobId === 'string' && args.jobId ? args.jobId : null
      const rows = jobId
        ? ((await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where id = $1 and agent_id = $2`, [jobId, agent.id])) as unknown as WorkbenchJob[])
        : ((await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where agent_id = $1 order by created_at desc limit 10`, [agent.id])) as unknown as WorkbenchJob[])
      // Running jobs get a FRESH clone URL each poll (app tokens expire ~1h);
      // gated jobs stay locked until a human approves from the ticket.
      const jobs = await Promise.all(
        rows.map(async ({ plan: _p, ...j }) =>
          j.status === 'started' ? { ...j, cloneUrl: await cloneUrl(j.repo), workdir: `/opt/data/workbench/jobs/${j.id}` } : j,
        ),
      )
      return { ok: true, value: { jobs } }
    }

    case 'merge_to_testing': {
      const jobId = String(args.jobId ?? '')
      const rows = (await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where id = $1 and agent_id = $2`, [jobId, agent.id])) as unknown as WorkbenchJob[]
      const job = rows[0]
      if (!job) return { ok: false, error: 'unknown job' }
      const r = await mergeJobToTesting(job, agent.model)
      if (!r.ok) return { ok: false, error: r.error }
      return { ok: true, value: { merged: true, testingBranch: r.testingBranch, note: 'Testing merge only — the PR still ships through review.' } }
    }

    case 'request_repo': {
      const cfg = await getGithubConfig()
      const org = String(args.org ?? '').trim()
      const repoName = String(args.name ?? '').trim().toLowerCase().replace(/[^a-z0-9._-]+/g, '-').slice(0, 100)
      if (!cfg.repoCreationOrgs.length) return { ok: false, error: 'repo creation is not enabled — an admin can approve orgs on the GitHub panel' }
      if (!cfg.repoCreationOrgs.includes(org)) return { ok: false, error: `org "${org}" is not approved for repo creation (approved: ${cfg.repoCreationOrgs.join(', ')})` }
      if (!repoName) return { ok: false, error: 'name required' }
      const taskId = typeof args.taskId === 'string' && args.taskId ? args.taskId : null
      // Same gate as start_job: this taskId is stored on the request row and
      // gets a workbench audit line, so it is authorised rather than believed.
      if (taskId) {
        const auth = await authorizeTicket(subject, taskId)
        if (!auth.ok) return auth
      }
      const dup = await sql`
        select 1 from workbench_repo_requests where agent_id = ${agent.id} and org = ${org} and name = ${repoName} and status = 'pending' limit 1
      `
      if (dup.length) return { ok: false, error: 'you already have a pending request for this repo — a human will decide' }
      await sql`
        insert into workbench_repo_requests (agent_id, agent_model, org, name, description, why, task_id)
        values (${agent.id}, ${agent.model}, ${org}, ${repoName}, ${String(args.description ?? '').slice(0, 300)}, ${String(args.why ?? '').slice(0, 1000)}, ${taskId})
      `
      await logTicket(taskId, agent.model, `requested a new repo: ${org}/${repoName} — awaiting human approval`)
      // Admins hear about it once, through the inbox — not per retry.
      void (async () => {
        const { addNotification } = await import('./notifications')
        const admins = (await sql`select id from users where role = 'admin'`) as unknown as Array<{ id: string }>
        for (const a of admins) {
          await addNotification(a.id, {
            kind: 'workbench-repo-request',
            title: `${agent.displayName} requests a new repo`,
            body: `${org}/${repoName} — ${String(args.why ?? '').slice(0, 120)}`,
            href: '/admin?tab=org',
          }).catch(() => {})
        }
      })().catch(() => {})
      return { ok: true, value: { status: 'pending', note: 'Request filed — a human decides. Continue other work; the repo appears in list_repos if approved.' } }
    }

    case 'finish_job': {
      const jobId = String(args.jobId ?? '')
      const rows = (await sql.unsafe(`select ${JOB_ROW} from workbench_jobs where id = $1 and agent_id = $2`, [jobId, agent.id])) as unknown as WorkbenchJob[]
      const job = rows[0]
      if (!job) return { ok: false, error: 'unknown job' }
      // Abandon works from ANY live state — including a still-gated plan.
      if (args.abandon === true && (job.status === 'started' || job.status === 'awaiting_approval')) {
        await sql`update workbench_jobs set status = 'abandoned', updated_at = now() where id = ${job.id}`
        await logTicket(job.taskId, agent.model, `workbench job abandoned: ${job.repo} @ ${job.branch}`)
        return { ok: true, value: { status: 'abandoned' } }
      }
      if (job.status === 'awaiting_approval') return { ok: false, error: 'the plan has not been approved yet — poll job_status' }
      if (job.status !== 'started') return { ok: false, error: `job is already ${job.status}` }
      const summary = String(args.summary ?? '').slice(0, 20_000)
      const base = await effectiveBase(job.repo)
      const ahead = await branchAhead(job.repo, base, job.branch)
      if (ahead === 0) return { ok: false, error: 'the branch has no commits yet — push your work first (or finish with abandon:true)' }
      let ticketLine = ''
      let t: Array<{ ticketRef: string | null; title: string }> = []
      // The ticket ref and TITLE go into a PUBLIC PR title and body, so the
      // board check is re-run at the disclosure point rather than trusted from
      // start_job: job rows written before that gate existed (or whose board
      // grant was revoked since) must not publish a ticket the agent may no
      // longer see. Losing the link degrades the PR; it never blocks the work.
      const ticketOk = job.taskId ? await ticketStillOurs(agent.model, job.taskId) : false
      if (job.taskId && ticketOk) {
        t = (await sql`
          select case when tk.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || tk.ticket_no end as "ticketRef", tk.title
          from tasks tk join boards b on b.id = tk.board_id where tk.id = ${job.taskId}
        `) as unknown as Array<{ ticketRef: string | null; title: string }>
        if (t[0]) ticketLine = `Ticket: ${t[0].ticketRef ?? job.taskId} — ${t[0].title}\n\n`
      }
      const body =
        `${ticketLine}${summary || '(no summary provided)'}` +
        (job.plan ? `\n\n## Plan\n\n${job.plan}` : '') +
        `\n\n---\n🔧 Opened by **${agent.displayName}** (\`${agent.model}\`) via the Talaria workbench (${job.effort} effort). Commits on this branch are authored by the agent.`
      const title = t[0] ? `${t[0].ticketRef ? `[${t[0].ticketRef}] ` : ''}${t[0].title}`.slice(0, 100) : `Workbench: ${job.branch}`
      const pr = await createPullRequest(job.repo, { head: job.branch, base, title, body })
      await sql`update workbench_jobs set status = 'pr_open', pr_url = ${pr.url}, summary = ${summary}, updated_at = now() where id = ${job.id}`
      await logTicket(ticketOk ? job.taskId : null, agent.model, `workbench PR opened: ${pr.url}`)
      return { ok: true, value: { prUrl: pr.url, prNumber: pr.number, note: 'Include this PR link in your outcome report.' } }
    }
  }
  return { ok: false, error: `unknown tool "${name}"` }
}

/** Merge a job's branch into the repo's testing branch — ONE implementation
 *  for both the agent verb and the human ticket-strip action. */
export async function mergeJobToTesting(
  job: Pick<WorkbenchJob, 'id' | 'repo' | 'branch' | 'status' | 'taskId'>,
  actor: string,
): Promise<{ ok: true; testingBranch: string } | { ok: false; error: string }> {
  if (job.status !== 'started' && job.status !== 'pr_open') return { ok: false, error: `job is ${job.status}` }
  const flow = await repoFlow(job.repo)
  if (!flow.testingBranch) return { ok: false, error: 'this repo has no testing branch configured — an admin can set one on the GitHub panel' }
  const r = await mergeInto(job.repo, flow.testingBranch, job.branch)
  if (!r.merged) return { ok: false, error: r.reason ?? 'merge failed' }
  const sql = await db()
  await sql`update workbench_jobs set merged_testing_at = now(), updated_at = now() where id = ${job.id}`
  await logTicket(job.taskId, actor, `workbench: merged ${job.branch} into ${flow.testingBranch} for testing${r.reason ? ` (${r.reason})` : ''}`)
  return { ok: true, testingBranch: flow.testingBranch }
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

/** `agent` takes the resolved AgentCaller where the caller has one — a bare
 *  model string still works (and is read as proven, which a legacy caller
 *  cannot exploit: it can never present a privileged name).
 *  FOLLOW-UP: routes/api/mcp.gw.$server.ts passes `name`; it holds `caller` two
 *  lines earlier and should pass that instead. Not this round's file. */
export async function dispatchWorkbenchMcp(
  rpc: Rpc,
  agent: AgentSubject,
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
        const r = await callTool(agent, tool.name, rpc.params?.arguments ?? {})
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
