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

`workbench_profiles` is a role-agnostic registry (the `dev` profile ships seeded: opencode, Pi, Oh My Pi). Per agent there is exactly one control, on the agent's Summary tab:

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

**Persistence:** harness session state (opencode storage, Pi / Oh My Pi agent dirs, the npm cache, Playwright browsers) lives on the department's state volume — surviving restarts and **shared across the department's agents**, so sessions can be resumed later or picked up by a teammate as a hand-off.

## Work sessions

Dispatch is not a single exchange. When a ticket enters an agent-start column with an agent assigned, Talaria pushes the work and then **keeps the session going** — continuation turns carrying live ticket status, up to 12 turns with 10 minutes of listening each — until the ticket reaches review/blocked/done. Agents work like a developer at a desk: run the harness, read its structured result, steer, test, repeat. UI work is verified **in a real browser** (Playwright) with screenshot evidence — "a UI change without a browser check is unverified" (see the `workbench-driving` canonical skill).

Behind the session sit the quality gates: plans (and the heavy-effort approval), the **QA judge** (enforcing by default when enabled — revise verdicts bounce the ticket straight back to the agent with the issues, capped at 3 revisions before a human takes over; pass/escalate always reach a human), and human sign-off as the only path to done.

### Concurrency and queueing

A workbench agent shares **one** container on **this VM**. Conversation-only agents stay cheap and uncapped. Coding work packs against the VM's `MemAvailable`:

- **VM keep-back** — 25% of `MemTotal`, clamped 2–16 GiB (`TALARIA_HOST_RESERVE` overrides). Kernel, docker, Postgres, Redis, and the app keep that RAM. Agents are `oom_score_adj: 500` so the kernel kills them first.
- **Push / pull** — a session or `start_job` is offered when free RAM can take the next job after that keep-back. Otherwise the ticket is **queued** (`work_wait`): the board card and ticket ticker say "queued · N ahead" / "next up — waiting for RAM" with the packing reason; the 60s sweep still starts it when a slot frees. Approving a heavy plan against a full VM toasts the same reason. Runaway guard is 16 jobs, not a desk size.
- **Ceiling** — each workbench container's hard `mem_limit` is `MemTotal − keep-back` (and never above 32 GiB). A leak OOMs **that agent**, not the VM. Reservation grows with live jobs; the ceiling does not shrink to the estimate.
- **Observability** — once-a-minute `docker stats` plus host pressure (`ok` / `tight` / `critical`) and a leak flag (RSS climbing ≥256 MiB/min for 8+ minutes past 2 GiB) on `GET /api/fleet/resources` and the run-detail Resources pane.

Packing floors: **conversation** 768 MiB / 2 GiB; **workbench idle** 2 GiB; **per job** 1 / 2 / 4 GiB (light / standard / heavy).

The 2026-09-17 dogfood freeze — fourteen tickets, eleven jobs, 4 GiB chassis, twenty-six OOM kills — was a hard `mem_limit` plus unbounded pile-up. Packing + a 32 GiB last-ditch ceiling replaces both the 3-job stall and the 8g one-size box.

Two known amplifiers stay out of packing, tracked separately: ever-growing session contexts (retried tickets can carry millions of tokens), and leftover per-job processes inside long-lived agent containers.


### Observing a run

The run-detail modal (every "watch the work" affordance opens it) is the full-insight view, one pane per question:

- **Live** — the agent's stream as it happens: its words, its reasoning, and every tool call *with its argument preview* (the preview carries the persona's display-redacted primary argument — the whole terminal command, which is where the harness steering is legible). Workbench MCP calls (`start_job`, `finish_job`, …) appear with full arguments and outcomes — they are platform-side, so their fidelity is total.
- **Turns** — the retained transcript: each turn's prompt and stream, captured to a `run-transcript` artifact on the ticket at turn end, scrubbed of known credential shapes, retained per `observability.transcriptRetentionDays` in admin settings (default 7 days; null = permanent).
- **Resources** — the agent container's cpu/memory/process sparklines over the run's window, sampled once a minute by the platform (admin-only; the same series feeds Observability → Compute).

What the live stream cannot show: tool RESULTS for tools that execute inside the agent container (the persona's completion frames don't carry them) — those land in the next turn's transcript only insofar as the agent's own reply reflects them. Richer container-side capture is a Hermes-side change, tracked as a follow-up.

## Harnesses: an open registry

A harness is a **declarative definition** (`defineWorkbenchHarness` in `@talaria/sdk/server` — the
old spelling `defineHarness` still builds, deprecated, because renaming an extension point out from
under third-party apps is a break rather than a rename; `defineHarness` now means the **activity**
contract in [`HARNESSES.md`](./HARNESSES.md)): auth (`'gateway'` → pointed at Talaria's gateway, metered and attributed; or `{provider, envVar}` → that provider's key from the org's endpoint registry), invocation templates (structured `jsonInvoke` strongly preferred — agents are taught to read structured results, never scrape logs), `mcpServe` for harnesses that can run *as* MCP servers (registered as stdio tools on the agent's own Hermes config), `mcpConfig` naming the pass-through format it reads (or `format: 'custom'` with a `renderMcpConfig` function, app-shipped only), a driving `guide`, a `probe`, and `install` hints (npm packages the workbench image preinstalls and `talaria-harness-update` refreshes). Builtins are **opencode**, **Pi**, and **Oh My Pi** — all gateway-auth, all invoked unattended (`npx @latest`, print/json mode, no TUI). Claude Code and Codex are not offered.

Three layers merge by slug (later wins): **builtins** ← **app-shipped** (`apps/<slug>/harness.ts`, enabled apps only) ← **admin-custom** JSON (`PUT /api/workbench/harnesses`). No host code ever runs from a definition; harness commands execute only inside the agent's sandbox. Builtins run via `npx` on the stock image — no custom image required; first use installs into the persistent cache. The agents' built-in browser is that pattern taken literally: its engine is fetched from npm on first use, which makes the chassis's pinned external resolvers (`AGENT_DNS_1`/`_2` in `fleet/.env`) a hard dependency — no DNS, no browser, and nothing in a health check to say so. For instant first-runs, build the **workbench image** (`scripts/build-workbench-image.sh` — Hermes chassis + preinstalled harnesses + Playwright/chromium) and set it on the profile. See "Shipping a harness" in [`APPS.md`](./APPS.md).

## Roadmap

Repo creation with human approval (approved-orgs allowlist) · multi-installation GitHub support · the role workbenches: data (real query/notebook execution, publish-gated), design (asset pipeline + browser rendering), publishing (CMS/email with human-gated sends), web-operator (allowlisted browser work with session audit artifacts).
