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
// transition lands in task_activity ON A TICKET STILL OPEN TO AGENTS: an audit
// line is a write to the ticket like any other, so it goes through the one gate
// (`logTicket` → `authorizeTicket`) rather than around it.
//
// THIS FILE TOUCHES A TICKET IN EXACTLY THREE WAYS, and each has ONE door:
//   · the ticket an agent NAMES in its arguments  → `ticketArg` (parse + gate,
//     one step, so `args.taskId` is read in one place in this file)
//   · an AUDIT LINE on that ticket                → `logTicket`
//   · the plan COMMENT and the plan artifact chip → `addComment` / `updateTask`
//     from server/tasks.ts, as the agent
// There is no fourth way and no raw `insert into task_…` / `update tasks` here.
// That is the whole invariant: every count of "how many doors" this file has
// ever carried was wrong within a round, because the doors were hand-written
// copies. These are not copies, so there is nothing to count.
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
        taskId: { type: 'string', description: 'The ticket this job implements — ALWAYS pass it when the work came from a ticket; it links the branch, audit trail, plan gate, and PR to the ticket. Refused if that ticket is one you may not work: a board you are not allowed on, a closed ticket (done / failed / cancelled), an archived ticket, or a ticket on an archived board. Ask for it to be reopened, or work the follow-up ticket.' },
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
        taskId: { type: 'string', description: 'The ticket that motivated it — same rule as start_job: a ticket you may not work (board not yours, closed, archived, archived board) is refused, so omit it rather than guessing.' },
      },
      required: ['org', 'name', 'why'],
    },
  },
  {
    name: 'finish_job',
    description:
      'Finish a job: Talaria verifies the branch has commits and opens the pull request with the ticket-linked body. Returns the PR URL — put it in your outcome report. Use abandon:true to close out a job that produced nothing. Either way the job closes out; the ticket only gets an audit line if it is still open to you (a ticket closed or archived while you worked takes no further agent writes), so report the PR URL yourself.',
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

/** Who a workbench audit line is written BY — and the reason `logTicket` takes
 *  this instead of a bare actor string.
 *
 *  A `task_activity` row on a ticket IS a write to that ticket. An AGENT's line
 *  therefore has to pass the same gate every other agent write passes; a
 *  HUMAN's (the ticket-strip approve / reject / merge-to-testing actions) does
 *  not, because the route already checked board edit rights.
 *
 *  `{ agent }` is the ONLY way to write an agent line, and it carries the
 *  subject the gate needs — there is no shape that says "an agent wrote this"
 *  and skips the check. The bare-string form is the human one, and it is
 *  VERIFIED rather than believed (see `agentBehind`): a string that names a
 *  fleet agent is treated as an agent write, so the human door cannot be used
 *  to launder an agent one back in. */
export type WorkbenchActor = string | { agent: AgentSubject }

/** The agent behind a WorkbenchActor, or null for a genuine human. Deliberately
 *  not `agentByModel` — that filters on `enabled`, and a disabled agent's writes
 *  must not fall through the gate by being read as a person's. */
async function agentBehind(by: WorkbenchActor): Promise<AgentSubject | null> {
  if (typeof by !== 'string') return by.agent
  const sql = await db()
  const rows = await sql`select 1 from agent_defs where model = ${by} limit 1`
  return rows.length ? by : null
}

/** THE ONLY door onto a ticket's workbench audit trail, and it asks the gate
 *  itself.
 *
 *  This used to take a bare actor string and write UNCONDITIONALLY, across five
 *  call sites of which two happened to sit behind `authorizeTicket`. The other
 *  three — `finish_job --abandon`, `finish_job`'s PR line, and
 *  `mergeJobToTesting` — wrote `task_activity` rows onto tickets a person had
 *  closed or archived, which is precisely what gating the ticket-taking verbs
 *  was supposed to end. The invariant had moved into the gate; the laundering
 *  had moved to the callers.
 *
 *  The fix is not five checks at five sites. It is that a caller can no longer
 *  EXPRESS the ungated write: every agent line goes through `authorizeTicket`
 *  here, once, and the actor type forces the caller to say which kind of write
 *  it is making.
 *
 *  Silence on refusal is deliberate: these lines are audit trail, never the
 *  operation. A refused line is skipped — the merge still merged, the PR is
 *  still open, the job is still abandoned, and the agent has all of that in its
 *  tool result. Returns whether the line landed, for callers that want to say so.
 *
 *  EXPORTED on purpose: it is the workbench's only ticket-audit door, and a new
 *  workbench surface reaching for `logActivity` directly is the next copy of
 *  this bug. Import this instead — it costs nothing and it cannot be wrong. */
export async function logTicket(taskId: string | null, by: WorkbenchActor, description: string): Promise<boolean> {
  if (!taskId) return false
  const subject = await agentBehind(by)
  if (subject !== null) {
    const auth = await authorizeTicket(subject, taskId)
    if (!auth.ok) return false
  }
  const actor = typeof by === 'string' ? by : subjectModel(by.agent)
  const { logActivity } = await import('./tasks')
  return logActivity(taskId, actor, 'workbench', description).then(
    () => true,
    () => false,
  )
}

/** THE GATE — for a caller-supplied taskId AND for every workbench audit line
 *  (`logTicket` calls this; nothing else may). Verbs here take the ticket from
 *  the agent, and everything downstream either DISCLOSES it (the ticket ref and
 *  title ride into the branch name and, at finish_job, into a public PR title
 *  and body) or WRITES to it (a plan comment authored as the agent, the plan
 *  artifact chip on the ticket, workbench audit lines). None of that reaches
 *  `updateTask`, so the two rules it carries are enforced here instead:
 *    · the board's agent policy must allow this agent (a ticket on a board the
 *      agent cannot see is not its work — and its title is not its business)
 *    · a ticket a person has taken off the table takes no further agent writes
 *  Both are `agentTicketRefusal`, IMPORTED — ONE predicate answers policy AND
 *  ticket state, so this door cannot ask half the question. It used to be a copy
 *  carrying only the closed-status third of it, so an agent could start a
 *  workbench job against an archived ticket — plan comment, artifact chip and
 *  all. Unknown and not-allowed refuse with the SAME message: a distinct "no
 *  such ticket" would turn this verb into a ticket enumeration oracle. */
async function authorizeTicket(subject: AgentSubject, taskId: string): Promise<{ ok: true } | { ok: false; error: string }> {
  const { getTask, agentTicketRefusal } = await import('./tasks')
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
  const shut = await agentTicketRefusal(task, subject, 'write')
  if (shut) {
    return {
      ok: false,
      error: `${shut}. Ticket ${task.ticketRef ?? taskId} is "${task.status}". Ask for it to be reopened, or work the follow-up ticket.`,
    }
  }
  return { ok: true }
}

/** THE ONLY way a verb gets a ticket out of its OWN arguments — parsing and the
 *  gate are one step, and `args.taskId` is read here and nowhere else in this
 *  file.
 *
 *  `authorizeTicket` being right did not make its CALLERS right: two verbs
 *  (`start_job`, `request_repo`) each hand-wrote the same three lines — coerce
 *  `args.taskId` to a string-or-null, then remember to authorize it — and the
 *  third verb to take a ticket argument would have hand-written them a third
 *  time. That is the shape every laundering path in this codebase has had: a
 *  correct predicate, re-expressed at each door, until one door expresses only
 *  part of it. Here the parse RETURNS the authorization, so a verb cannot hold
 *  an agent-supplied taskId it has not gated — the ungated read is not a thing
 *  you can write, rather than a thing you must remember not to write.
 *
 *  Absent/blank taskId is `{ ok: true, taskId: null }`: omitting the ticket is
 *  legal on both verbs, and only a NAMED ticket is a claim to be checked. */
async function ticketArg(
  subject: AgentSubject,
  args: Record<string, unknown>,
): Promise<{ ok: true; taskId: string | null } | { ok: false; error: string }> {
  const taskId = typeof args.taskId === 'string' && args.taskId ? args.taskId : null
  if (!taskId) return { ok: true, taskId: null }
  const auth = await authorizeTicket(subject, taskId)
  return auth.ok ? { ok: true, taskId } : auth
}

/** Board policy alone, for the DISCLOSURE point (finish_job): may this agent
 *  still see this ticket? Deliberately NOT the closed check — a person closing
 *  the ticket while the job ran should not cost the PR its ticket link. */
async function ticketStillOurs(subject: AgentSubject, taskId: string): Promise<boolean> {
  const { getTask } = await import('./tasks')
  const task = await getTask(taskId).catch(() => null)
  if (!task) return false
  const { boardAllowsAgent } = await import('./boards')
  return boardAllowsAgent(task.boardId, subject)
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
      if (!(await grantedRepos(agent.id)).includes(repo)) return { ok: false, error: `repo "${repo}" is not granted to you — list_repos shows yours` }
      if (effort !== 'light' && !plan.trim()) return { ok: false, error: 'standard/heavy work requires a plan first — describe approach, files, and test strategy in `plan`' }
      // BEFORE anything reads the ticket: the caller supplied this id, so board
      // policy and the closed-ticket rule decide whether it may be touched at
      // all. Everything below (ref + title in the branch name, the plan comment
      // and artifact chip on the ticket, the audit line, and finish_job's public
      // PR title) depends on this having passed — which is why the id arrives
      // ALREADY gated rather than gated on the next line.
      const ticket = await ticketArg(subject, args)
      if (!ticket.ok) return ticket
      const taskId = ticket.taskId
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
        // Through `addComment`, not `insert into task_comments`. This was the
        // last raw ticket write left in this file, and it was the same shape as
        // the bug above it: correct today only because `ticketArg` happens to
        // run first. Going through the comment door means it also lands in the
        // activity feed and pushes the board — which a raw insert silently did
        // not, so a plan comment appeared on the ticket with nothing in its
        // history saying it had.
        const { addComment } = await import('./tasks')
        await addComment(taskId, agent.model, `**Workbench plan** (${effort} effort · ${repo}):\n\n${plan}`).catch(() => {})
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
        { agent: subject },
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
      const r = await mergeJobToTesting(job, { agent: subject })
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
      // Same door as start_job — literally the same function, not the same
      // three lines again: this taskId is stored on the request row and gets a
      // workbench audit line, so it is authorised rather than believed.
      const ticket = await ticketArg(subject, args)
      if (!ticket.ok) return ticket
      const taskId = ticket.taskId
      const dup = await sql`
        select 1 from workbench_repo_requests where agent_id = ${agent.id} and org = ${org} and name = ${repoName} and status = 'pending' limit 1
      `
      if (dup.length) return { ok: false, error: 'you already have a pending request for this repo — a human will decide' }
      await sql`
        insert into workbench_repo_requests (agent_id, agent_model, org, name, description, why, task_id)
        values (${agent.id}, ${agent.model}, ${org}, ${repoName}, ${String(args.description ?? '').slice(0, 300)}, ${String(args.why ?? '').slice(0, 1000)}, ${taskId})
      `
      await logTicket(taskId, { agent: subject }, `requested a new repo: ${org}/${repoName} — awaiting human approval`)
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
      // Abandon works from ANY live state — including a still-gated plan. The
      // job row is this agent's own (the select filters on agent_id), so the
      // state change needs no ticket permission; the AUDIT LINE does, and
      // logTicket asks for it. Abandoning was the one branch gated by nothing
      // at all, and it is how an agent used to write onto a closed ticket.
      if (args.abandon === true && (job.status === 'started' || job.status === 'awaiting_approval')) {
        await sql`update workbench_jobs set status = 'abandoned', updated_at = now() where id = ${job.id}`
        await logTicket(job.taskId, { agent: subject }, `workbench job abandoned: ${job.repo} @ ${job.branch}`)
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
      const ticketOk = job.taskId ? await ticketStillOurs(subject, job.taskId) : false
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
      // `ticketOk` above answered the DISCLOSURE question (may this ref and
      // title go into a public PR?). The audit line asks a different one — may
      // this agent still WRITE to the ticket? — so it is not re-derived here:
      // logTicket asks the gate. A ticket closed while the job ran therefore
      // gets no line, which is the invariant working: the PR still opened and
      // its URL is in the result the agent gets back.
      await logTicket(job.taskId, { agent: subject }, `workbench PR opened: ${pr.url}`)
      return { ok: true, value: { prUrl: pr.url, prNumber: pr.number, note: 'Include this PR link in your outcome report.' } }
    }
  }
  return { ok: false, error: `unknown tool "${name}"` }
}

/** Merge a job's branch into the repo's testing branch — ONE implementation
 *  for both the agent verb and the human ticket-strip action, which is exactly
 *  why `by` is a WorkbenchActor and not an actor string: the two callers are
 *  not the same kind of writer, and the audit line it emits has to know. The
 *  human route (`routes/api/workbench.jobs.ts`) passes `actorOf(user)` — an
 *  email — and is unaffected; the agent verb passes `{ agent: subject }` and is
 *  gated. The merge itself is a GitHub operation and proceeds either way. */
export async function mergeJobToTesting(
  job: Pick<WorkbenchJob, 'id' | 'repo' | 'branch' | 'status' | 'taskId'>,
  by: WorkbenchActor,
): Promise<{ ok: true; testingBranch: string } | { ok: false; error: string }> {
  if (job.status !== 'started' && job.status !== 'pr_open') return { ok: false, error: `job is ${job.status}` }
  const flow = await repoFlow(job.repo)
  if (!flow.testingBranch) return { ok: false, error: 'this repo has no testing branch configured — an admin can set one on the GitHub panel' }
  const r = await mergeInto(job.repo, flow.testingBranch, job.branch)
  if (!r.merged) return { ok: false, error: r.reason ?? 'merge failed' }
  const sql = await db()
  await sql`update workbench_jobs set merged_testing_at = now(), updated_at = now() where id = ${job.id}`
  await logTicket(job.taskId, by, `workbench: merged ${job.branch} into ${flow.testingBranch} for testing${r.reason ? ` (${r.reason})` : ''}`)
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
