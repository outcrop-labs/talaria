# The Workbench

The Workbench is how Talaria's agents do **real execution work** — starting with software development — inside sandboxes scoped to their role, under lifecycle rules the platform owns. The persona (a Hermes agent) stays the judgment layer: it reads tickets, plans, communicates, and reviews. The workbench is its power tool.

It's a **reproducible methodology**, not a dev-only feature. Every workbench is the same six pieces; the dev workbench is simply the first instance (data, design, publishing, and web-operator workbenches ride the same chassis later):

1. **A runtime profile** — image + env + mounts + preinstalled harnesses, composed into the agent's container by the fleet renderer.
2. **Scoped credentials** — only what the role touches (per-repo GitHub access for dev), never god tokens.
3. **A governed toolkit** — the workbench MCP: the risky lifecycle (branches, PRs, merges) is platform-owned; agents drive it with tools.
4. **Effort→model routing** — agents pick *effort*; Talaria picks the *model*.
5. **MCP pass-through** — the agent's existing MCP grants, rendered into each harness's native config. Nothing reconnects inside a sandbox.
6. **The audit spine** — every job transition lands in the ticket's activity next to dispatch, judge, and review events.

## Profiles and THE setting

`workbench_profiles` is a role-agnostic registry (the `dev` profile ships seeded: opencode, Claude Code, Codex CLI, Oh My Pi). Per agent there is exactly one control, on the agent's Summary tab:

> **Workbench: Off / Auto / On** (+ optional explicit profile)

*Auto* attaches by fit rules declared on the profile (departments/roles — e.g. engineering, or a "Data Engineer" role). *On* forces a profile. *Off* means no sandbox. The tab shows a live "→ resolves to" readout, plus the per-agent tuning:

- **Harness dropdown** — which coding tool this agent drives (Auto = the profile's first).
- **Low / Medium / High model selects** — what each effort level means *for this agent* (blank = the org-wide Workbench model roles on /models, resolved with a fall-down chain so unset slots never strand a job).

## GitHub, connected once

Admin → Org → **GitHub · Workbench**: connect via a **GitHub App** (recommended — short-lived installation tokens, per-repo installs) or a **fine-grained PAT**. The *Setup guide* modal walks every field on GitHub's actual forms. Secrets seal via secretbox and never render back; status live-verifies.

Connecting grants nothing by itself. **Repo access is an explicit per-agent grant** (toggle chips on the agent), validated against the connection's reachable pool. Per-repo **flow** config sets which branch PRs land on (blank = the repo's default) and an optional **testing branch** — features can be merged into it for integration testing (agent verb or ticket button), but testing merges never replace review: the PR still ships normally.

## The job lifecycle (why git never gets messy)

Agents **never run raw git against origin**. The workbench MCP (a Talaria-owned server in the MCP registry, granted per agent like any capability) owns the lifecycle:

- `doctor` — end-to-end self-diagnosis: profile, chosen harness (+ a probe command to verify the binary), auth, GitHub, repo grants, effort map, config paths.
- `list_repos` — the agent's granted repos + the effort→model map.
- `start_job(repo, taskId, effort, plan)` — Talaria cuts `talaria/<ticket-ref>-<slug>` from the flow's base branch, records the job (one live job per ticket), and returns a short-lived authenticated clone URL, a **per-job workspace** (`/opt/data/workbench/jobs/<id>` — concurrent jobs never collide), the resolved model, and per-harness invocation lines with the model in each harness's own syntax. **Plans are required for standard/heavy effort**, post to the ticket as a comment *and* as a markdown artifact, and **heavy jobs wait for human approval** from the ticket's workbench strip before any clone URL exists.
- `job_status` — jobs with fresh clone URLs (tokens expire by design).
- `merge_to_testing(jobId)` — into the repo's testing branch, when configured.
- `finish_job(jobId, summary)` — verifies the branch has real commits, then opens the PR with a templated ticket-linked body (title from the ticket ref, plan + summary inside, the acting agent named). `abandon: true` closes out a dead job from any live state.

**Attribution:** commits are authored as the agent (`Analyst (Talaria agent) <analyst-engineering@agents.talaria.local>` — provisioned git identity per sandbox), so history and blame show who did the work. API-level actions (branch/PR/merge) show the App's identity; PR footers name the acting agent.

**Persistence:** harness session state (Claude Code sessions, opencode storage, Codex home, the npm cache, Playwright browsers) lives on the department's state volume — surviving restarts and **shared across the department's agents**, so sessions can be resumed later or picked up by a teammate as a hand-off.

## Work sessions

Dispatch is not a single exchange. When a ticket enters an agent-start column with an agent assigned, Talaria pushes the work and then **keeps the session going** — continuation turns carrying live ticket status, up to 12 turns with 10 minutes of listening each — until the ticket reaches review/blocked/done. Agents work like a developer at a desk: run the harness, read its structured result, steer, test, repeat. UI work is verified **in a real browser** (Playwright) with screenshot evidence — "a UI change without a browser check is unverified" (see the `workbench-driving` canonical skill).

Behind the session sit the quality gates: plans (and the heavy-effort approval), the **QA judge** (enforcing by default when enabled — revise verdicts bounce the ticket straight back to the agent with the issues, capped at 3 revisions before a human takes over; pass/escalate always reach a human), and human sign-off as the only path to done.

## Harnesses: an open registry

A harness is a **declarative definition** (`defineWorkbenchHarness` in `@talaria/sdk/server` — the
old spelling `defineHarness` still builds, deprecated, because renaming an extension point out from
under third-party apps is a break rather than a rename; `defineHarness` now means the **activity**
contract in [`HARNESSES.md`](./HARNESSES.md)): auth (`'gateway'` → pointed at Talaria's gateway, metered and attributed; or `{provider, envVar}` → that provider's key from the org's endpoint registry), invocation templates (structured `jsonInvoke` strongly preferred — agents are taught to read structured results, never scrape logs), `mcpServe` for harnesses that can run *as* MCP servers (Claude Code, Codex — registered as stdio tools on the agent's own Hermes config), `mcpConfig` naming the pass-through format it reads (or `format: 'custom'` with a `renderMcpConfig` function, app-shipped only), a driving `guide`, a `probe`, and reserved `install` hints.

Three layers merge by slug (later wins): **builtins** ← **app-shipped** (`apps/<slug>/harness.ts`, enabled apps only) ← **admin-custom** JSON (`PUT /api/workbench/harnesses`). No host code ever runs from a definition; harness commands execute only inside the agent's sandbox. Builtins run via `npx` on the stock image — no custom image required; first use installs into the persistent cache. For instant first-runs, build the **workbench image** (`scripts/build-workbench-image.sh` — Hermes chassis + preinstalled harnesses + Playwright/chromium) and set it on the profile. See "Shipping a harness" in [`APPS.md`](./APPS.md).

## Roadmap

Repo creation with human approval (approved-orgs allowlist) · multi-installation GitHub support · the role workbenches: data (real query/notebook execution, publish-gated), design (asset pipeline + browser rendering), publishing (CMS/email with human-gated sends), web-operator (allowlisted browser work with session audit artifacts).
