# The Workbench

The Workbench is how Talaria's agents do **real execution work** — starting with software development — inside sandboxes scoped to their role, under lifecycle rules the platform owns. The persona (a Hermes agent) stays the judgment layer: it reads tickets, plans, communicates, and reviews. The workbench is its power tool.

The routing rule that makes the workbench the only path: **agents never do dev work in chat**. Code changes — writing, modifying, or committing code, however small — require a ticket and a workbench job. When dev work is asked for in a chat, the agent's job is to create or link the ticket, post the chat's instructions to it as comments, and move execution into a workbench; even an explicit out-of-band request routes through a ticket first.

It's a **reproducible methodology**, not a dev-only feature. Every workbench is the same six pieces; the dev workbench is simply the first instance (data, design, publishing, and web-operator workbenches ride the same chassis later):

1. **A sandbox**: the dev overlay (env, state dirs, git identity, memory ceiling) and its coding harness, composed into the agent's container by the fleet renderer.
2. **Scoped credentials** — only what the role touches (per-repo GitHub access for dev), never god tokens.
3. **A governed toolkit** — the workbench MCP: the risky lifecycle (branches, PRs, merges) is platform-owned; agents drive it with tools.
4. **Model roles**: the org's Workbench roles become the harness's own roles; the harness picks between them as it works.
5. **MCP pass-through** — the agent's existing MCP grants, rendered into each harness's native config. Nothing reconnects inside a sandbox.
6. **The audit spine** — every job transition lands in the ticket's activity next to dispatch, judge, and review events.

## The Developer Agent switch

Per agent there is exactly one control, on the agent's Summary tab:

> **Developer Agent: on / off**

On sets the agent up end to end, and nothing else needs wiring: the dev sandbox, **Oh My Pi** as its coding harness, and the Workbench tools (`doctor`, `start_job`, `finish_job`, …). Flipping it rolls the agent so the change lands. The switch is the *only* grant of the Workbench MCP server: the registry derives it from `agent_defs.developer` and ignores assignment or team rows for it, and the MCP page shows the Workbench as "every Developer Agent" with no access controls. Which repos the agent may touch stays an explicit pick, shown under the switch once it is on.

Oh My Pi is the only harness. There is no profile registry, no per-agent harness pick, and no per-agent effort→model table. Models come from the org-wide **Workbench model roles** on /models: `code-standard` is omp's default (the `--model` on every invocation line), `code-light` its `smol` role, and `code-heavy` its `slow` and `plan` roles (rendered as `PI_SMOL_MODEL` / `PI_SLOW_MODEL` / `PI_PLAN_MODEL`). omp moves between them on its own (subagents and small steps on smol, the advisor on slow, plan mode on plan), so a job is not pinned to one model. Unset roles fall down (heavy → standard → light → utility), so a missing slot never strands a job.

## GitHub, connected once

Admin → Org → **GitHub · Workbench**: connect via a **GitHub App** (recommended — short-lived installation tokens, per-repo installs) or a **fine-grained PAT**. The *Setup guide* modal walks every field on GitHub's actual forms. Secrets seal via secretbox and never render back; status live-verifies.

Connecting grants nothing by itself. **Repo access is an explicit per-agent grant** (toggle chips on the agent), validated against the connection's reachable pool. Per-repo **flow** config sets which branch PRs land on (blank = the repo's default) and an optional **testing branch** — features can be merged into it for integration testing (agent verb or ticket button), but testing merges never replace review: the PR still ships normally.

## The job lifecycle (why git never gets messy)

Agents **never run raw git against origin**. The workbench MCP (a Talaria-owned server in the MCP registry, granted by the Developer Agent switch) owns the lifecycle:

- `doctor`: end-to-end self-diagnosis: the Oh My Pi harness (+ a probe command to verify the binary), GitHub, repo grants, omp's model roles, config paths.
- `list_repos`: the agent's granted repos + omp's model roles.
- `start_job(repo, taskId, effort, plan)`: Talaria cuts `talaria/<ticket-ref>-<slug>` (or, when the repo grant carries a configured branch prefix, `<prefix>/<ticket-ref>-<slug>`, named to satisfy the repo's own branch rules so its pushes are accepted) from the flow's base branch, records the job (one live job per ticket), and returns a short-lived authenticated clone URL, a **per-job workspace** (`/opt/data/workbench/jobs/<id>`, so concurrent jobs never collide), omp's model roles, and the Oh My Pi invocation lines (default model filled in). Effort decides planning and how much RAM the job reserves, not the model. **Plans are required for standard/heavy effort**, post to the ticket as a comment *and* as a markdown artifact, and **heavy jobs wait for human approval** from the ticket's workbench strip before any clone URL exists.
- `job_status` — jobs with fresh clone URLs (tokens expire by design).
- `merge_to_testing(jobId)` — into the repo's testing branch, when configured.
- `finish_job(jobId, summary)` — verifies the branch has real commits, then opens the PR with a templated ticket-linked body (title from the ticket ref, plan + summary inside, the acting agent named). `abandon: true` closes out a dead job from any live state.

**Teardown is the platform's, not the agent's.** When `finish_job` opens the PR, Talaria stops every process still running in the job's workdir (the checkout stays for a revise bounce). Abandoning removes the workdir outright. The `workbench-job-sweep` (every 10 minutes) covers what never calls a verb: a `started` or awaiting-approval job whose ticket reached a done column or was archived is abandoned, and it and any `pr_open` job on a closed ticket lose their workdir. Jobs with no ticket are left alone. `workspace_cleared_at` marks a job whose workdir is gone.

A cap refusal and a "no commits yet" refusal name the job id, branch, and workdir — finish or push there; retrying the same call does not change the answer. `jobId` is the uuid `start_job` returned, not a ticket ref. Omitting `effort` means `standard`, and `standard`/`heavy` are refused without `plan`.

Git in a job workdir asks Talaria for a credential. A checkout outside that workdir only gets one when git names the repo — the helper reads `origin` when git omits the path. `git credential fill` prints the token into the transcript; do not run it.

**Attribution:** commits are authored as the agent (`Analyst (Talaria agent) <analyst-engineering@agents.talaria.local>` — provisioned git identity per sandbox), so history and blame show who did the work. API-level actions (branch/PR/merge) show the App's identity; PR footers name the acting agent.

**Persistence:** harness session state (Oh My Pi's agent dir, the npm cache, Playwright browsers) lives on the department's state volume, surviving restarts and **shared across the department's agents**, so sessions can be resumed later or picked up by a teammate as a hand-off.

## Work sessions

Dispatch is not a single exchange. When a ticket enters an agent-start column with an agent assigned, Talaria pushes the work and then **keeps the session going** — continuation turns carrying live ticket status, up to 12 turns with 10 minutes of listening each — until the ticket reaches review/blocked/done. Agents work like a developer at a desk: run the harness, read its structured result, steer, test, repeat. UI work is verified **in a real browser** (Playwright) with screenshot evidence — "a UI change without a browser check is unverified" (see the `workbench-driving` canonical skill).

Behind the session sit the quality gates: plans (and the heavy-effort approval), the **QA judge** (enforcing by default when enabled — revise verdicts bounce the ticket straight back to the agent with the issues, capped at 3 revisions before a human takes over; pass/escalate always reach a human), and human sign-off as the only path to done.

The chat boundary is policy, not habit: agents never write, modify, or commit code from a chat thread, no exceptions for small changes. Dev work requires a ticket and a workbench job — a dev-work request in chat is answered by creating or linking the ticket and moving execution into the workbench, even when a human explicitly asks for an out-of-band change, and dev-work instructions given in chat are captured as comments on the relevant ticket. The rule ships in every rendered soul (the dev-policy header) and in the fleet-wide talaria-toolkit skill.

### Concurrency and queueing

A workbench agent shares **one** container on **this VM**. Conversation-only agents stay cheap and uncapped. Coding work packs against the VM's `MemAvailable`:

- **VM keep-back** — 25% of `MemTotal`, clamped 2–16 GiB (`TALARIA_HOST_RESERVE` overrides). Kernel, docker, Postgres, Redis, and the app keep that RAM. Agents are `oom_score_adj: 500` so the kernel kills them first.
- **Push / pull** — a session or `start_job` is offered when free RAM can take the next job after that keep-back. Otherwise the ticket is **queued** (`work_wait`): the board card and ticket ticker say "queued · N ahead" / "next up — waiting for RAM" with the packing reason; the 60s sweep still starts it when a slot frees. Approving a heavy plan against a full VM toasts the same reason. Runaway guard is 16 jobs, not a desk size.
- **Ceiling** — each workbench container's hard `mem_limit` is `MemTotal − keep-back` (and never above 32 GiB). A leak OOMs **that agent**, not the VM. Reservation grows with live jobs; the ceiling does not shrink to the estimate.
- **Observability** — once-a-minute `docker stats` plus host pressure (`ok` / `tight` / `critical`) and a leak flag (RSS climbing ≥256 MiB/min for 8+ minutes past 2 GiB) on `GET /api/fleet/resources` and the run-detail Resources pane.

Packing floors: **conversation** 768 MiB / 2 GiB; **workbench idle** 2 GiB; **per job** 1 / 2 / 4 GiB (light / standard / heavy).

The 2026-09-17 dogfood freeze — fourteen tickets, eleven jobs, 4 GiB chassis, twenty-six OOM kills — was a hard `mem_limit` plus unbounded pile-up. Packing + a 32 GiB last-ditch ceiling replaces both the 3-job stall and the 8g one-size box.

One known amplifier stays out of packing, tracked separately: ever-growing session contexts (retried tickets can carry millions of tokens). Leftover per-job processes and workdirs are the job teardown's (above).


### Observing a run

The run-detail modal (every "watch the work" affordance opens it) is the full-insight view, one pane per question:

- **Agent** — Hermes replies and its own tool calls, retained turns above the live tail. A terminal call that invokes the coding harness is not shown here.
- **Turns** — each harness exchange: the ask the agent sent, the harness output, and the prose the agent wrote back before it did anything else.
- **Resources** — the agent container's cpu/memory/process sparklines over the run's window, sampled once a minute by the platform (admin-only; the same series feeds Observability → Compute).

What the live stream cannot show on its own: tool RESULTS for tools that execute inside the agent container (the persona's completion frames don't carry them). The **talaria-events** Hermes plugin closes that gap: rendered into every agent, it reports each tool call's name, arguments, and result (clamped, secret-scrubbed at the api boundary) to `POST /api/agents/tool-events`, which lands `toolfull` frames on the live run's watch stream — so results appear live and in the retained transcripts. Correlation is the agent's newest live work session (a documented v1 approximation; exact persona-session pinning is a follow-up).

## The harness: Oh My Pi

The workbench runs one coding harness, **Oh My Pi** (omp), defined once in `api/crates/talaria-workbench-harnesses`. It authenticates through Talaria's gateway on the workbench credential (metered and attributed), keeps its config and sessions in `PI_CODING_AGENT_DIR` on the department state volume, reads the agent's MCP grants from a rendered `mcp.json`, and is invoked unattended (`npx @latest`, print/json mode, `--auto-approve`, no TUI). Claude Code, Codex, OpenCode and Pi are not offered; their skill names are signposts pointing at Oh My Pi.

omp runs via `npx` on the stock image, no custom image required; first use installs into the persistent cache. The agents' built-in browser is that pattern taken literally: its engine is fetched from npm on first use, which makes the chassis's pinned external resolvers (`AGENT_DNS_1`/`_2` in `fleet/.env`) a hard dependency: no DNS, no browser, and nothing in a health check to say so. For instant first-runs, build the **workbench image** (`scripts/build-workbench-image.sh`: Hermes chassis + Oh My Pi + Playwright/chromium) and set `TALARIA_WORKBENCH_IMAGE` in the app's env; Developer Agents render on it.

## Roadmap

Repo creation with human approval (approved-orgs allowlist) · multi-installation GitHub support · the role workbenches: data (real query/notebook execution, publish-gated), design (asset pipeline + browser rendering), publishing (CMS/email with human-gated sends), web-operator (allowlisted browser work with session audit artifacts).
