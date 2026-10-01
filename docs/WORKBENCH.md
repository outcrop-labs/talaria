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

Oh My Pi is the only harness. There is no profile registry, no per-agent harness pick, and no per-agent effort→model table. Models come from the org-wide **Workbench model roles** on /models: `code-standard` is omp's default (the `--model` on every invocation line), `code-light` its `smol` role, and `code-heavy` its `slow` and `plan` roles (rendered as `PI_SMOL_MODEL` / `PI_SLOW_MODEL` / `PI_PLAN_MODEL`). omp moves between them on its own (subagents and small steps on smol, the advisor on slow, plan mode on plan), so a job is not pinned to one model. Unset roles fall down (heavy → standard → light → utility), so a missing slot never strands a job. An agent with a **coding account** signed in runs its harness on that subscription's models instead — see [Coding accounts](#coding-accounts-the-harness-on-your-own-subscription).

## Coding accounts: the harness on your own subscription

By default a workbench harness reaches models through Talaria's gateway on the
workbench credential, metered and attributed. **Coding accounts** are the other
option: sign an agent in to a coding subscription — Claude Pro/Max, ChatGPT
Codex, GitHub Copilot, Gemini, Cursor, Z.AI, and the rest of omp's roster — and
its harness runs on that account instead.

What this does *not* change: the agent itself. The persona driving the harness
keeps the model its agent def configures, through the gateway, metered as
always. Coding accounts move the **harness's** models and nothing else.

The one consequence worth saying out loud: a harness run on a subscription does
not pass through the gateway, so **it does not land in Talaria's ledger**. The
provider's own dashboard is the record of that spend.

### Turning it on

`Admin → Agents → Coding accounts` carries two controls, and they are two on
purpose. The first enables the feature. The second is the **allowlist**: which
services this org permits. "Developers may sign agents in to coding
subscriptions" and "to anything omp supports" are different decisions, and an
org that wants Copilot and Codex but not a personal Claude Max subscription
needs to be able to say exactly that. While nothing is permitted, nobody can
sign anything in.

Removing a service stops its accounts driving any harness immediately — jobs
fall back to the gateway — but **keeps the credentials**, so re-permitting it
needs no fresh sign-in. Throwing a credential away takes someone pressing
*Sign out*.

### Signing in

On the agent's Summary tab, under the Developer Agent switch: one account per
service, per agent, signed in by anyone who can edit that agent (the
`agents.manage` permission, or owning a personal assistant). Picking the same
service again replaces the account rather than adding a second one, even
through a different door — `openai-codex` and its headless device variant are
one subscription reached two ways.

The flows are **omp's own**, run by a bridge beside the api
([`omp-auth/`](../omp-auth/README.md)): nothing in Talaria encodes a provider's
client id, PKCE quirk or token endpoint. Three shapes come out of it, and the
sign-in dialog is one state machine over all three:

- **a link to open** — most authorization-code providers. They redirect to
  `http://localhost:<port>` on the machine running the flow, which is the
  instance. On a dev stack (same machine as your browser) the redirect
  completes the login by itself; on a deployed instance it lands nowhere, and
  you paste the code or the failed redirect URL back instead. That fallback is
  the provider's own, declared in omp's auth rules.
- **a link and a user code** — device-code providers. Approve it and the dialog
  finishes on its own; there is nothing to paste.
- **a question first** — some providers ask for an enterprise domain, a region
  or a login method before they hand over a link.

### Who holds the credential

**This instance does.** The whole credential, refresh token included, seals with
secretbox into Postgres and shows up in `Admin → Secrets` like every other
sealed secret. The agent's omp reads it over omp's own **auth-broker protocol**,
served at `/api/workbench/auth/v1/*` and scoped by the agent's own key — so an
agent reads exactly its own accounts and nothing else.

A snapshot hands the harness an access token with `__remote__` where the refresh
token would be, which is what makes Talaria the only thing that can refresh one:
when the token expires, omp calls back and the bridge runs the provider's own
refresher. A credential the provider declares dead is disabled with its reason,
stops being served, and says "sign in again" in the UI rather than retrying for
ever. `POST /v1/credential` — the protocol's upload — is **refused by design**:
logins are a human action in the UI, and a sandbox that could write its own
credential could grant itself a subscription.

A volume reset cannot lose these, and nothing OAuth-shaped ever reaches an
agent's config files.

### Which plan runs a job, and on which model

A **plan** is either one of the agent's coding accounts or the Talaria gateway.
The gateway is a *choice*, not only a fallback: an agent with subscriptions
signed in can still be told to run some or all of its coding work through the
gateway, on models the org picked. Three layers decide, narrowest first:

1. **the ticket's pin** — this ticket on that plan, optionally naming one
   model;
2. **the agent's default plan** — what a job that pins nothing gets. No account
   marked default means the gateway;
3. **nothing configured** — the org's Workbench model roles, exactly as before
   this feature existed.

Role picks hang off the **plan**, not the agent, and that is the point: a plan
is a coherent set of models, so swapping plans swaps the set with it instead of
leaving `smol` on the subscription that just ran out. Within a plan, a role it
does not fill inherits its `default`; a plan that names nothing leaves the org's
roles standing.

**The per-ticket pin** is on the ticket, above the workbench jobs, because a
subscription runs out mid-ticket and the fix has to be local: move *this* ticket
onto the other account — and name the model too, since the other plan's flagship
is not always what the first plan's default role would have chosen — without
re-pointing the agent and every other ticket with it.

`start_job` resolves the plan per job and puts it **on the invocation line**:
`--model <provider>/<model>` plus inline `PI_SMOL_MODEL` / `PI_SLOW_MODEL` /
`PI_PLAN_MODEL`. Container env can only ever carry the agent's default (it is
rendered per agent, not per ticket), so the line is what makes a pinned plan —
and an account signed in since the last roll — actually take effect. `doctor`
reports which plan the agent codes on, so an agent debugging an unexpected model
has something to read.

## GitHub, connected once

Admin → Org → **GitHub · Workbench**: connect via a **GitHub App** (recommended — short-lived installation tokens, per-repo installs) or a **fine-grained PAT**. The *Setup guide* modal walks every field on GitHub's actual forms. Secrets seal via secretbox and never render back; status live-verifies.

Connecting grants nothing by itself. **Repo access is an explicit per-agent grant** (toggle chips on the agent), validated against the connection's reachable pool. Per-repo **flow** config sets which branch PRs land on (blank = the repo's default). Talaria's job ends at the pull request: how that branch is then reviewed, promoted or merged belongs to the repository's own branch protection and CI, not to anything Talaria assumes.

## The job lifecycle (why git never gets messy)

Agents **never run raw git against origin**. The workbench MCP (a Talaria-owned server in the MCP registry, granted by the Developer Agent switch) owns the lifecycle:

- `doctor`: end-to-end self-diagnosis: the Oh My Pi harness (+ a probe command to verify the binary), GitHub, repo grants, omp's model roles, config paths.
- `list_repos`: the agent's granted repos + omp's model roles.
- `start_job(repo, taskId, effort, plan)`: Talaria cuts `talaria/<ticket-ref>-<slug>` (or, when the repo grant carries a configured branch prefix, `<prefix>/<ticket-ref>-<slug>`, named to satisfy the repo's own branch rules so its pushes are accepted) from the flow's base branch, records the job (one live job per ticket), and returns a short-lived authenticated clone URL, a **per-job workspace** (`/opt/data/workbench/jobs/<id>`, so concurrent jobs never collide), omp's model roles, and the Oh My Pi invocation lines (default model filled in). Effort decides planning and how much RAM the job reserves, not the model. **Plans are required for standard/heavy effort**, post to the ticket as a comment *and* as a markdown artifact, and **heavy jobs wait for human approval** from the ticket's workbench strip before any clone URL exists.
- `prepare_env(jobId)`: sets up the job's dev environment after the clone (see "Dev environments" below).
- `job_status` — jobs with fresh clone URLs (tokens expire by design).
- `finish_job(jobId, summary)` — verifies the branch has real commits, then opens the PR with a templated ticket-linked body (title from the ticket ref, plan + summary inside, the acting agent named). `abandon: true` closes out a dead job from any live state.

**Teardown is the platform's, not the agent's.** When `finish_job` opens the PR, Talaria stops every process still running in the job's workdir (the checkout stays for a revise bounce). Abandoning removes the workdir outright. The `workbench-job-sweep` (every 10 minutes) covers what never calls a verb: a `started` or awaiting-approval job whose ticket reached a done column or was archived is abandoned, and it and any `pr_open` job on a closed ticket lose their workdir. Jobs with no ticket are left alone. `workspace_cleared_at` marks a job whose workdir is gone.

A cap refusal and a "no commits yet" refusal name the job id, branch, and workdir — finish or push there; retrying the same call does not change the answer. `jobId` is the uuid `start_job` returned, not a ticket ref. Omitting `effort` means `standard`, and `standard`/`heavy` are refused without `plan`.

Git in a job workdir asks Talaria for a credential. A checkout outside that workdir only gets one when git names the repo — the helper reads `origin` when git omits the path. `git credential fill` prints the token into the transcript; do not run it.

**Attribution:** commits are authored as the agent (`Analyst (Talaria agent) <analyst-engineering@agents.talaria.local>` — provisioned git identity per sandbox), so history and blame show who did the work. API-level actions (branch/PR/merge) show the App's identity; PR footers name the acting agent.

**Persistence:** harness session state (Oh My Pi's agent dir, the npm cache, Playwright browsers) lives on the department's state volume, surviving restarts and **shared across the department's agents**, so sessions can be resumed later or picked up by a teammate as a hand-off.

## Dev environments

Agents run unprivileged in a stock image, so the platform sets up each job's toolchain rather than leaving the agent to hand-install rustup and linkers into its home. `prepare_env(jobId)`, called right after the clone:

- **Toolchains through [mise](https://mise.jdx.dev)**, in user space on the persistent volume, so a department's agents share one download of each version. A repo's own `mise.toml` / `.tool-versions` is used as is. Otherwise the versions come from the files each ecosystem already keeps: `rust-toolchain.toml` (plus mold when `.cargo/config.toml` links with it), `.nvmrc` / `.node-version` / `package.json` (node, and bun or pnpm from `packageManager` or the lockfile), `go.mod`, `.python-version` / `pyproject.toml` / `uv.lock`, `.ruby-version`, at the repo root or one directory down. Detected tools go to a `mise.local.toml` that `.git/info/exclude` keeps out of commits.
- **OS packages through apt, as root**, only for names the repo lists in `.talaria/workbench.toml`, and only from the image's Debian sources. The agent itself never gets root.
- **mise's shims on PATH** in the agent's login shells, so `cargo`, `bun`, `go` and the rest resolve to the repo's pinned versions by directory.
- **A shared Rust compile cache.** For any repo with a `Cargo.toml`, sccache becomes cargo's compiler wrapper, with one cache per department at `/opt/data/workbench/harness/sccache` (20 GiB cap). Every job still starts from an empty `target/`, but a dependency any job already compiled comes out of the cache instead of being rebuilt, so only the first job after a dependency change pays for the full graph. It's scoped to the repo through `mise.local.toml`'s `[env]`, never set globally. Opt out with `sccache = false` in `.talaria/workbench.toml`.

```toml
# .talaria/workbench.toml (optional)
apt = ["libssl-dev", "protobuf-compiler"]
sccache = false   # opt a Rust repo out of the shared compile cache

[tools]           # extra mise tools, merged over detection
protoc = "28"
```

It is idempotent and cheap once versions are cached, so agents call it on every job. A missing tool is fixed by declaring it in the repo, never by the agent installing it. Talaria's own `mise.toml` is the example: the same file gives people and agents Rust, Bun, Node, and mold.

## Keeping a job's disk bounded

Job teardown answers *"is this job over?"* — `finish_job` stops the builds, abandoning removes the workdir, and the sweep catches whatever never called a verb. What nothing asked was *"how big is this?"* A job that is perfectly alive — `started`, ticket open, agent working — holds a checkout plus whatever that project's toolchain writes beside it, and a cold Rust `target/` is 4–20 GiB on its own. Ten live jobs in a department is 200 GiB of entirely legitimate work, and the first notice is a build that fails on ENOSPC.

Two things have to be true to fix that for **any** project, not just the ones Talaria happens to know. Somebody has to measure, because growth nothing reports is growth nobody sees until it is an outage. And somebody has to know what is *rebuildable* — the platform cannot guess that `target/` costs a cargo build while `.venv` cost twenty minutes and a private index. Only the project knows that.

### The third rung: reclaim

| | Processes | Build artifacts | Checkout | When |
|---|---|---|---|---|
| **Stop** | killed | kept | kept | `finish_job` — the PR is open, a revise bounce still needs the tree |
| **Reclaim** | untouched | **dropped** | kept | an idle job, or the volume over budget |
| **Remove** | killed | dropped | dropped | abandoned, or the ticket closed |

**Reclaim** deletes only what the project has said it can rebuild. The checkout, the branch, uncommitted edits and `.git` all survive, so a job that is reclaimed and then resumed pays a rebuild and nothing else — and with the department's shared sccache behind it, a Rust rebuild is mostly cache hits rather than a cold graph.

### What a project declares

`.talaria/workbench.toml` is already where a repo tells the workbench about itself, so cleanup joins it rather than earning a second file:

```toml
[cleanup]
# Rebuildable. Dropped from an idle job before its checkout is ever touched.
# These MERGE OVER detection — name what detection would miss.
artifacts = ["target", "dist", ".turbo"]

# Expensive to recreate: a private-index virtualenv, a downloaded model, a
# fixture corpus. Never dropped, whatever detection thinks, whatever the
# pressure. `keep` always wins over `artifacts`.
keep = [".venv", "fixtures/corpus"]

# This repo's ceiling for ONE job's workdir. The platform default applies
# when unset. A job over its ceiling is reclaimed at the next idle check
# rather than waiting for the volume to get tight.
maxWorkdirGib = 40
```

Every path is **repo-relative**. An absolute path, a `..` segment, an empty segment, or `.git` at any depth is refused and reported back through `doctor` — this list ends in an `rm -rf` inside a container, so it gets the same discipline as `job_workdir`: validated in Rust, validated again in the script, and never handed to a shell on trust.

### What a project gets for free

A repo that declares nothing still gets sane behavior, from the same marker scan `prepare_env` already runs:

| Detected | Treated as rebuildable |
|---|---|
| `Cargo.toml` | `target` |
| `package.json` | `node_modules/.cache`, `dist`, `build`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo` |
| `pyproject.toml`, `requirements.txt`, `.python-version` | `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache` |
| `pom.xml` | `target` |
| `build.gradle`, `build.gradle.kts` | `build`, `.gradle` |
| `*.sln`, `*.csproj` | `bin`, `obj` |
| `mix.exs` | `_build` |

What is **absent** matters as much as what is there. `node_modules` itself is rebuildable only by going back to the network, so only its cache is dropped. `vendor/` is often committed and load-bearing offline. A Go build cache lives in `GOCACHE` outside the repo, so it is capped as a department cache instead of reclaimed per job. The rule throughout: *cheap and local* is reclaimable, *slow or remote* is not.

Opting out is one line — `artifacts = []` — and `keep` overrides any single detected entry, and the subtree beneath it, without discarding the rest. Detection is on by default because a project that must opt in mostly will not, and then the disk fills anyway.

### When reclaim fires

Never against a job that might be building. There is no idle *timer* to tune and get wrong: the reclaim script checks `/proc` for a live process whose cwd is inside the workdir — the same predicate `Stop` already uses, inverted — and answers `BUSY` without touching anything. A job compiling at that moment is simply asked again on the next sweep.

The existing ten-minute job sweep now runs two passes. The **status pass** is the old one, asking *"is this job over?"*. The **size pass** asks what it never could:

- **Per-job ceiling.** Every live job (`started`, `awaiting_approval`, `pr_open`) is measured. One over its ceiling — its repo's `maxWorkdirGib`, or the platform's 25 GiB default — is reclaimed, because a single job is not entitled to the whole disk.
- **Volume budget.** Once the filesystem holding the workbench volume is **80%** full, any live job with something to give back is reclaimed, oldest first, whatever its ceiling. This is the backstop for the case the per-job rule cannot help with: several genuinely active jobs, none individually outrageous, that together fill the disk.

Oldest-first matters: the job most likely to be picked up again keeps its cache longest.

Everything in the size pass is best-effort and per job. A container it cannot reach, a workdir with no checkout, a repo with no policy, or a busy build is skipped — never retried in a tight loop, and never allowed to fail the sweep. The platform does not guess on a repo it could not read.

Every reclaim is logged with the job, the paths dropped and the bytes recovered, and the sweep's own summary line carries the total. Putting it on the ticket's activity alongside the other job transitions — where the audit spine already lives, and where a deletion belongs — is not wired yet.

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

The workbench runs one coding harness, **Oh My Pi** (omp), defined once in `api/crates/talaria-workbench-harnesses`. It authenticates through Talaria's gateway on the workbench credential (metered and attributed) unless the agent has a coding account signed in (see above), keeps its config and sessions in `PI_CODING_AGENT_DIR` on the department state volume, reads the agent's MCP grants from a rendered `mcp.json`, and is invoked unattended (`npx @latest`, print/json mode, `--auto-approve`, no TUI). Claude Code, Codex, OpenCode and Pi are not offered; their skill names are signposts pointing at Oh My Pi.

omp runs via `npx` on the stock image, no custom image required; first use installs into the persistent cache. The agents' built-in browser is that pattern taken literally: its engine is fetched from npm on first use, which makes the chassis's pinned external resolvers (`AGENT_DNS_1`/`_2` in `fleet/.env`) a hard dependency: no DNS, no browser, and nothing in a health check to say so. For instant first-runs, build the **workbench image** (`scripts/build-workbench-image.sh`: Hermes chassis + Oh My Pi + Playwright/chromium) and set `TALARIA_WORKBENCH_IMAGE` in the app's env; Developer Agents render on it.

## Roadmap

Repo creation with human approval (approved-orgs allowlist) · multi-installation GitHub support · the role workbenches: data (real query/notebook execution, publish-gated), design (asset pipeline + browser rendering), publishing (CMS/email with human-gated sends), web-operator (allowlisted browser work with session audit artifacts).
