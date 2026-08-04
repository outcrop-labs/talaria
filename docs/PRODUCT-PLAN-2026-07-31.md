# Talaria — product gap remediation plan

_2026-07-31 · the build plan for [`PRODUCT-GAPS-2026-07-31.md`](./PRODUCT-GAPS-2026-07-31.md) ·
against `1ff47b0`_

Eleven milestones closing G1–G10, sequenced by dependency rather than by desirability. Every
milestone names where it lands in the tree, what it depends on, and a verifiable done-signal.

**One structural note up front, because it changes the order from the audit.** The audit ranked the
gaps by value ÷ effort and put the value ledger (G1) second. It can't go second. Cost-per-approved-
ticket needs two things that are currently broken rather than missing: correct per-ticket cost
attribution, and an "approved" signal an agent can't forge. Both are P0s in
[`AUDIT-2026-07-30.md`](./AUDIT-2026-07-30.md). Building the scorecard first means shipping a
dashboard that is confidently wrong — worse than no dashboard, because people will make decisions on
it. So the value ledger moves behind the gate, and the two unblocked items (notifications, spend
caps) move ahead of it.

---

## Phase 0 — The gate

Not re-planned here; [`AUDIT-2026-07-30.md`](./AUDIT-2026-07-30.md) already has a numbered task list.
But five of its tasks are **hard dependencies** for this plan, and the rest of this document assumes
they're done:

| Audit task | Why this plan needs it |
|---|---|
| **1, 2** — migration ordering + `schema_migrations` | Every milestone below adds tables. Adding migrations to a system where fresh installs already fail at statement 21 compounds a P0 into an unrecoverable one. **Nothing here starts before this.** |
| **16** — metering fixes | M4's central number is cost per approved ticket. Today `work-dispatch.ts:124` passes `refId: task.id` and not `taskId` — and `UsageInput` (`usage.ts:8`) declares both fields, so this is a one-line fix. Workbench harness runs and Hermes crons are unmetered entirely. Without this the scorecard's cost column is near-empty and silently wrong. |
| **11** — HITL invariant into `updateTask` | M4's "approved first-pass" metric is meaningless while an agent can close its own ticket via `status: cancelled`. The scorecard would measure agent honesty, not agent quality. |
| **10** — per-agent credentials | Landed for the **API** surface: `agent-auth.ts` resolves identity from the agent's own `tak_` credential and treats `x-agent-name` as a cross-check that can never grant access. This is what makes board policy, MCP allowlists and retrieval principals enforceable. |
| **per-agent gateway keys** — SEPARATE, still unbuilt | **Not** covered by task 10, and this is the dependency M2's per-agent spend caps and M4's per-agent attribution actually have. The LLM gateway is outside that change entirely: `fleet-brain.ts:25,97` renders the *same* `fleet-gateway` key into every container as `${LLM_API_KEY}`, and `llm.v1.chat.completions.ts:34` attributes spend to `api:${id.keyName}` with no agent identity in the picture. Metering therefore still cannot tell two agents apart. **M2 and M4 ship org/key-scoped first and gain per-agent scope when per-agent gateway keys land.** |
| **37** — backups | Prerequisite for honestly selling anything in PRICING.md. Pairs with M0. |

**Estimated: 2–3 weeks.** Everything below is blocked on tasks 1–2; individual milestones note their
other dependencies.

### Phase 0 status — what actually shipped

Recorded against the working tree on 2026-07-31, so the plan below can be read without re-deriving
what is done. Row by row, against the table above:

| Gate item | State | Evidence |
|---|---|---|
| **1, 2** migration ordering | **shipped** | `db/pg.ts:1212-1266` — `schema_migrations` keyed by array index, per-statement checksum, per-statement transaction, boot refused if an applied statement changed. Fresh installs no longer die at statement 21. |
| **11** HITL in `updateTask` | **shipped; the duplicated predicate is now centralized** | `agentSafePatch` (`server/tasks.ts`) carries the whole invariant — archived ticket, archived board, closed ticket, out of review, out of blocked, stranded status, and the assignment gate at its foot that a *coerced* terminal move falls through to. `updateTask` applies it whenever `who.kind === 'agent'`, and `HumanApprovalRequired` becomes a 403 in `routes/api/tasks.$id.ts`. An agent can no longer close its own ticket via `status: cancelled`, restart its own blocked work, take anything out of review, or write to a closed ticket. **Four** agent-reachable side doors never reach `updateTask` and enforce the closed-ticket rule themselves — `tasks.$id.usage.ts`, `tasks.$id.dependencies.ts`, `agent.gap.ts` (gap on a closed ticket → 403, with the fix in the message: re-send without `taskId`), and `authorizeTicket` in `server/workbench-mcp.ts` (board policy **and** closed-ticket, on every verb that takes a caller-supplied `taskId`). Count **four** when auditing coverage: an earlier version of this row listed only the first two, which read as if the other pair were unguarded. All four now import `agentTicketRefusal` from `server/tasks.ts` instead of hand-rolling it — see the note below the table for why there were four copies and what that cost. That predicate has since absorbed the board's **agent policy** as well, so the pull side (`assignedWork`), the push side (`maybeDispatchTicket`) and the live work-session loop answer the same question the write routes answer. |
| **16** metering | **partly shipped** | the dispatch turn's `recordUsage` call in `work-dispatch.ts` now passes `taskId` as well as `refId`, so dispatch turns reach the ticket's cost rollup. Workbench harness runs are metered — they hold their own `workbench-gateway` key and only the persona key is in `gateway_unmetered_keys` (`fleet-brain.ts:175-191`). **Hermes crons remain unmetered: zero usage rows**, and it is not fixable from the cron surface (`agent-crons.ts:7-16` — a cron turn is byte-identical to a persona turn at the gateway; closing it means Talaria drives the schedule through `proxyChat`). |
| **10** per-agent credentials | **shipped for the API surface, deploy is ordered** | `agent-auth.ts` resolves identity from the agent's own `tak_` credential. Deploying it is **not** a restart: migrate → render → roll → *then* flip `TALARIA_AGENT_KEY_LEGACY=off`, and a personal/elevated assistant is refused outright between deploy and roll. Runbook: [AGENT-KEY-MIGRATION.md](./AGENT-KEY-MIGRATION.md). |
| **per-agent gateway keys** | **still unbuilt — unchanged** | See below. |
| **37** backups | **shipped, not scheduled** | `scripts/backup.sh` / `scripts/restore.sh` / [BACKUPS.md](./BACKUPS.md). Nothing schedules it (cron is the operator's job until M1's scheduler), and the restore drill has never actually been run — both called out in `PRICING.md`. |
| **M0** truth in copy | **shipped** | `README.md` no longer claims "and what it shipped"; `PRICING.md` marks backups and SSO as not-yet-sellable with the specific gaps. |

**Audit task 11 took six rounds, and the sixth was self-inflicted. Worth recording, because the
cause was the process and not the code.**

Rounds one through five each found a *laundering path* — a way for an agent to reach a terminal or
assigned state that `agentSafePatch` was supposed to forbid — and each round closed the one it found:
the two-write self-assignment through an intermediate status (gating moved from the source column to
the destination); `status: ''` slipping past `patch.status &&` as falsy and blanking the column with
every guard skipped (presence, not truthiness, plus a board-membership check in `updateTask` ahead of
it); a terminal move coerced into the review catch and then *returning* instead of falling through to
the assignment gate; `handoffTarget` guessing a done or agent-start column when the board had no
hand-off column (it now refuses); and a review key that had been recategorised to `done` passing a
post-condition that only asserted the key existed. Five patches, five rounds, and each round found
another — which was the signal that the method was wrong, not that the code was unusually leaky.

The sixth path was different in kind: it was **created by how the work was split**. Rounds were
parallelised by file ownership, so an agent that did not own `server/tasks.ts` could not export
anything from it and copy-pasted the closed-ticket predicate instead. That happened four times
(`tasks.$id.usage.ts`, `tasks.$id.dependencies.ts`, `agent.gap.ts`, `workbench-mcp.ts`), and the
copies did not track the original as it grew its archival clauses — so the invariant was duplicated
rather than centralized, and a seventh path was only ever a matter of the next route being added by
whoever did not own the file. All four copies carried a `FOLLOW-UP` comment asking for exactly the
fix this round makes.

**This round therefore removes surface area instead of adding a guard.** `server/tasks.ts` now
exports one `agentTicketRefusal(task, agent, intent, facts?)` — returning the *reason* an agent may not act, or null —
and all four local copies are deleted. Everything that has to ask "may an agent still write to this
ticket?" imports it and none of them restates it: `agentSafePatch` (the patch gate), the four side
doors, and **both dispatch sides** — `maybeDispatchTicket` on the push side and `assignedWork` on the
heartbeat pull side.

Two real defects fell out of the consolidation rather than being separately hunted, which is the
argument for doing it this way:

- The four copies each carried only the closed-status third of the rule, so an agent could log spend,
  add a dependency, file a gap or run a workbench verb against a ticket a person had **archived**, or
  a ticket on an **archived board**. Importing the predicate closed that in all four at once.
- `assignedWork` selected on `agent_start` alone, so the heartbeat **handed agents work every write
  route would then refuse** — archived tickets, tickets on archived boards, tickets parked in a
  done-category or off-board-keyed pickup column — and the agent looped on them every heartbeat with
  no way to make progress. The query now selects candidates and lets the predicate decide, rather
  than restating three conditions in SQL as a sixth copy.

Returning the reason string rather than a boolean is deliberate: the refusal reads the same sentence
wherever the write arrived, and a new caller cannot invent a vaguer one.

**Still open, plainly:**

- The consolidation is a refactor, not a proof. Nothing in CI fails a route that hand-rolls the
  predicate again — the structural fix rests on the import being the obvious path and on review.
  A lint rule (or a test that enumerates agent-reachable ticket writes) is the real close-out.
- There is **no regression test** for any of the six paths. Every one of them was found by reading,
  and would be found by reading again.
- Rounds 1–5's fixes carry forward unchanged — the archival and closed-status clauses now *delegate*
  to the shared predicate, and nothing else in `agentSafePatch` moved — and they remain unretested.
  This round did not revisit them.
- `updateTask` is the choke point for *ticket patches*. It is not a choke point for everything an
  agent can do to a ticket — the four side doors exist because writes that attach to a ticket
  (usage rows, dependencies, gap reports, workbench plan comments and PR titles) legitimately do not
  go through it. Consolidating the predicate makes them consistent; it does not make them one path.

**Per-agent gateway keys are the one gate item that has not moved, and it is the one M2 and M4
actually depend on.** Audit task 10 gave the *API* surface per-agent identity; the LLM gateway is a
different plane and still meters by API key:

- Every agent container renders the same `GATEWAY_KEY_NAME = 'fleet-gateway'` credential as
  `${LLM_API_KEY}` (`fleet-brain.ts`).
- `recordGatewayUsage` (`llm-gateway.ts`) writes `usage_events.agent_model = caller`, and `caller` is
  `` `api:${id.keyName}` `` (`routes/api/llm.v1.chat.completions.ts`). So workbench spend —
  the largest single line item, a coding run outweighing a day of chat — lands in the ledger
  attributed to the **pseudo-agent `api:workbench-gateway`**, not to the agent that drove the run,
  and with no `task_id` at all.
- Hermes cron spend lands nowhere.

Concretely: today the ledger can tell you what the workbench cost, and cannot tell you which agent
spent it. **M2's `agent` budget scope and M4's per-agent cost column stay blocked on this**; both ship
at org/key scope first, exactly as their sections say. Nothing else in Phase 0 gates them any more.

---

## M0 — Tell the truth in the copy

**SHIPPED** — kept here for the record, and because the rule outlives the three edits.

Three claims outran the code. All three are now fixed in the working tree:

- `README.md` — the costs bullet no longer claims "and what it shipped". The claim comes back when
  M4 ships the scorecard that would make it true.
- `docs/PRICING.md:35` — SSO at Business tier now carries the `*` that points at the
  "Not sellable yet" block. Un-star it at M11.
- `docs/PRICING.md:33` — backups at Starter, same treatment: starred, with the specific gap named
  (nothing schedules `scripts/backup.sh`, and the restore drill has never been run).

Cheap, and it removes the worst failure mode for a pre-revenue product: a prospect discovering the
gap themselves. **The standing rule is the deliverable:** a tier feature gets a `*` and a named gap
until the code behind it exists, and the star comes off in the same change that closes the gap.

---

## Phase 1 — Close the loop with the human

The three milestones that make already-shipped autonomy safe to actually turn on.

### M1 — Notification delivery + digest (G2)

**Goal.** An approval, escalation, or block reaches a person who is not in the tab.

**Why first.** Every autonomy feature already shipped — crons, outreach sweeps, multi-turn work
sessions, judge escalation — is currently throttled by this. It's the highest-leverage unblocked
work, and it becomes the fan-out point M6's webhooks reuse.

**What ships.**

1. **A real scheduler** — `server/scheduler.ts`, one `setInterval` registered at boot from
   `server-entry.js`. Today the three background jobs (`maybeSweepIdleChats`, `maybeOutreachSweep`,
   the price refresh) are opportunistic throttled kicks off `routes/api/channels.ts:32-36`. That
   pattern cannot carry a daily digest: a quiet instance serves no requests, and a quiet instance is
   exactly when a digest matters most. Migrate the existing three onto the scheduler as well — this
   fixes a live latent bug where an idle instance never decays comms or sweeps outreach.
2. **Per-user preferences** — `alter table users add column notify_prefs jsonb not null default '{}'`.
   Event classes: `mention · dm · approval_pending · judge_escalation · agent_blocked ·
   gap_reported · work_complete`. Each routes to in-app / email / both.
3. **Fan-out at the existing choke point.** `notifications.ts:16` is already the single writer.
   Extend it: write the row → resolve prefs → enqueue email. No call site changes.
4. **Daily digest** — `server/digest.ts`. Reuses `homeSummary()` (`home.ts:48`), which *already*
   computes each user's triage / review / blocked queues in one pass. Render through the existing
   `emailShell()` (`email.ts:102`) → `sendEmail()`. The email transport is already built, sealed, and
   admin-configurable; it's just only wired to `invites.ts` today.
5. **Approval escalation.** A `google_pending_actions` row (`pg.ts:758`) or a `quality_review` ticket
   aging past a configurable threshold nags its owner, then escalates to an admin. This is what turns
   the HITL gate from a bottleneck into an SLA.

**Lands in:** `server/scheduler.ts` (new) · `server/digest.ts` (new) · `server/notifications.ts` ·
`server/email.ts` · `server-entry.js` · settings panel in `routes/_app/settings.tsx`.

**Depends on:** audit tasks 1–2.

**Done when:** a user with email prefs on receives a digest on an instance that has served zero HTTP
requests for 24 hours, and an approval left pending past threshold escalates to an admin.

**Size: M.**

---

### M2 — Spend caps and budgets (G3)

**Goal.** A buyer can answer "what's the worst case?" with a number instead of a shrug.

**What ships.**

1. **`spend_budgets`** — `(scope 'org'|'key'|'agent', scope_id, monthly_usd, on_breach
   'warn'|'pause'|'block')`.
2. **`spend_period`** — a counter table `(scope, scope_id, period, usd)` incremented inside
   `recordUsage()` (`usage.ts:64`). Deliberately *not* a live aggregate over `usage_events`: the check
   sits in the hot path of every generation, and repricing the month per request would be the most
   expensive query in the product.
3. **The check.** `routes/api/llm.v1.chat.completions.ts`, after `authenticateKey` (line 18) and
   before `fetchUpstream` (line 53). `authenticateKey` already returns `{keyId, keyName, userId,
   email}` (`llm-keys.ts:69`), so org- and key-scoped budgets work today.
4. **Breach behavior.** `warn` = alert only · `pause` = stop non-interactive work (crons, outreach,
   workbench) while human-facing chat keeps working · `block` = hard stop. **`pause` is the default**
   — it's the correct behavior and the one that demos well, because the failure is visible without
   being catastrophic.
5. **Threshold alerts** at 50/80/100% into the existing `computeAlerts()` plane (`alerts.ts:31`).
6. **Run-rate forecast** on `/observability → Cost`, straight off the existing `perDay` series.

**Depends on:** M1 (breach notifications need delivery) · **per-agent gateway keys** for the `agent`
scope — not audit task 10, which gave the API surface per-agent identity and left the gateway on one
shared `fleet-gateway` key. **Ship `org` + `key` scope without waiting for that** — that alone bounds
the worst case, which is the entire point.

**Done when:** an org at 100% of budget has its cron and workbench traffic refused at the gateway
while a human's chat still streams, and the admin got mail at 80%.

**Size: M.**

---

### M3 — Mobile approvals surface (G5)

**Goal.** Unblock the desk-time ceiling on agent throughput.

**Explicitly not:** making seven work surfaces responsive. That's an L-sized project with a poor
return. Build one narrow surface well.

**What ships.**

1. **A responsive approvals view** — the three `homeSummary` queues, `google_pending_actions`
   confirm-sends, the QA gate, blocked agents, and mentions. Enough context to decide, and the two
   buttons.
2. **Mobile nav** — the one shared change: `nav-rail.tsx` is a fixed `w-56` with no collapse. Add a
   drawer under `md`.
3. **PWA** — manifest + service worker for installability. Web push after, once M1's event classes
   have proven out.

**Depends on:** M1 (a notification with nowhere good to land is worse than none).

**Done when:** a phone can take an agent's ticket from `quality_review` to done, and approve a
confirm-send, from a notification deep-link.

**Size: M.**

---

## Phase 2 — Prove the value

### M4 — Value ledger and agent scorecard (G1)

**Goal.** Answer "is this agent worth it?" with data the platform already holds.

**Why it's here and not in Phase 1.** See the note at the top. This milestone is a *reporting* build
on top of instrumentation that must be trustworthy first.

**What ships.**

1. **`server/scorecard.ts`** — per agent, per period, over tables that already exist:
   - tickets carried to review — `tasks` + `task_activity` status transitions
   - approved first-pass vs. bounced — `judge_reviews.verdict` sequence per `task_id`
   - mean revisions, judge verdict mix — `judge_reviews` (written at `judge.ts:154`, currently read
     back only per-ticket at `judge.ts:48`; this is the first aggregate consumer)
   - dispatch → review cycle time — `task_activity.created_at` deltas
   - capability gaps reported — `gaps.ts`
   - spend, and **cost per approved ticket** — `usage_events.task_id`
2. **Board rollups** — throughput over time, WIP, aging work, human vs. agent completion split.
3. **Two surfaces** — a `Workforce` tab on `/observability`, and a per-agent panel in the agent
   manage modal so the scorecard sits where you'd manage the agent.

**Performance note.** Start with live queries and measure. If the aggregates get slow, add a nightly
rollup table via M1's scheduler — but don't pre-build it; the data volumes here are small and a
rollup table that drifts is worse than a slow query.

**Depends on:** audit tasks 11, 16 (hard) · **per-agent gateway keys** (for per-agent attribution to
exist at all — audit task 10 did not deliver this; the gateway meters by API key, not by agent).

**Done when:** an admin can compare two agents on cost per approved ticket and first-pass rate over
the last 30 days, and the numbers reconcile against a hand-audited sample of tickets.

**Size: M** for the build. The dependency chain is what makes it feel large.

---

## Phase 3 — Adoption

### M5 — Activation path + base agent roster (G8, part of G7)

**Goal.** Shorten "cloned the repo" → "an agent replied to me" to under ten minutes.

**What ships.**

1. **First-run checklist on Home** — provider → org → first agent → first ticket dispatched, with
   real progress state in `app_settings`. Doubles as the activation metric, which currently doesn't
   exist in any form.
2. **Seeded demo board** with sample tickets on a fresh instance; one click to remove.
3. **Base agent roster** — canonical agent definitions shipped as JSON, mirroring how
   `scripts/skills/` already seeds with pristine-tracking (`.seeds.json`, TODO.md:45). Hiring the
   first agent becomes a pick from a gallery, not an authoring exercise. Reuses `fleet-create.ts`
   wholesale. This is also the cheap half of G7.
4. **Reorder the quick start** so the payoff comes before the configuration depth. Right now
   `README.md`'s path is four config steps before any value, with a hard dependency in the middle —
   no provider key means nothing in the product does anything.

**Depends on:** nothing. Could run parallel with Phase 1 if there's capacity.

**Done when:** a clean install reaches a streamed agent reply in under ten minutes without opening
the docs.

**Size: M.** Highest leverage per hour in this document for the OSS funnel.

---

### M6 — Import, export, webhooks (G4)

**Goal.** Remove the largest trial objection and back the anti-lock-in positioning with a button.

**What ships.**

1. **CSV/JSON ticket import** with a column mapper, on the board.
2. **Linear / Jira importers** — against their APIs, falling back to their CSV exports.
3. **Full-org JSON export** — admin-triggered, runs on M1's scheduler, lands as an artifact.
   `PRODUCT.md:57` sells "convenience, not captivity"; this is what makes that sentence true.
4. **Outbound webhooks** — `webhooks (url, secret, events[])`, dispatched from M1's fan-out point.
   Near-free given M1, and it covers the long tail (Zapier, n8n, ops channels) that would otherwise
   need connectors built one at a time.
5. **Per-user API tokens** — generalize the existing `llm_api_keys` / `tlk_` pattern
   (`llm-keys.ts:58`) into a scoped Talaria API token.

**Depends on:** M1 for the webhook dispatcher.

**Done when:** a Linear export lands as a populated board, and an admin can download the whole org
and re-import it into a clean instance.

**Size: S each, M for the set.**

---

## Phase 4 — Surface completeness

### M7 — Cross-surface search (G6)

**Goal.** Give the humans the retrieval the agents already have.

**What ships.** A `⌘K` palette over tickets, docs, artifacts, plans, messages, people, and agents,
backed by a unified `/api/search` — Postgres FTS plus the existing retrieval plane for semantic hits,
ACL-filtered per surface through the existing `canRead` / board-membership helpers.

**The sequencing win:** `server/tasks.ts` has no search function of any kind today — no `ilike`, no
`tsquery`. Building it here also closes the deferred agent-side gap (TODO.md:175, "ticket search by
title (cross-board)"). One implementation, two consumers: the human palette and a new MCP tool.

**Done when:** a ticket is findable by a word in its title from anywhere in the app, and from an
agent's toolkit, with results correctly ACL-filtered for a member who can't see the board.

**Size: M.**

---

### M8 — Cycles and recurring tickets (G6)

**What ships.**

1. **`cycles`** `(board_id, name, starts_at, ends_at, status)` + `tasks.cycle_id`. Board filter, a
   cycle panel, and a "this cycle" dimension on M4's scorecard — which is where the reporting gets
   legible, since a scorecard reads far better as "this cycle" than as a rolling 30 days.
2. **`recurring_tickets`** `(board_id, template jsonb, rrule, next_run_at)`, materialized by M1's
   scheduler.

**Why recurring matters more here than in a human PM tool.** Standing weekly work is precisely what
you want to hand an agent. Per-agent Hermes crons exist but are agent-side: recurring work currently
has no ticket, no board presence, no template, no QA gate, and no audit trail. A recurring *ticket*
routes through the entire governed lifecycle that's already built.

**Depends on:** M1 (scheduler). Pairs naturally with M4.

**Size: M.**

---

### M9 — Guest access (G9)

**Goal.** Let an agency put a client on a board without a seat or a public URL.

**What ships.** A guest principal: no seat, invited per board / plan / doc / artifact, sees only what
it's granted, and never the member directory or any other surface. Largely composition — the
viewer/editor grant model from KB and artifacts already exists, and the permission plane already does
per-person view gating. The work is extending grants to boards and plans, and adding the
directory/surface exclusions.

**Size: M**, lower than it sounds.

---

## Phase 5 — Bets

Pick deliberately. Not both at once, and not before Phases 1–3 land.

### M10 — Second workbench role: support (G7)

The largest TAM expansion in the product, and the hardest thing in this plan. **Support** over
marketing because it's *measurable* — bounded actions, an obvious quality gate that maps onto the
existing judge, and countable outcomes, which means it feeds M4 its best case study rather than
needing a new evaluation story.

Needs: an inbound channel (email or form intake), a conversation-shaped work item, a resolution gate,
and a role-scoped sandbox with support tooling instead of git. The harness registry (`defineHarness`)
and the job lifecycle generalize; the role scoping does not exist yet.

**This should get its own design document before any code.** **Size: L.**

### M11 — Generic OIDC (G10)

The cheap 80% of the enterprise gate — one adapter covers Okta, Entra, Auth0, and Keycloak.
`server/auth/config.ts` is already a genuinely pluggable, independently-enableable provider registry;
this is a third provider (`auth/oidc.ts`) mirroring `google.ts`, plus discovery and PKCE. SAML and
SCIM wait for a customer who names them.

**Size: S–M.**

---

## Sequence

Assumes a small team. Sizes are relative; the calendar is indicative, not a commitment.

```
Phase 0  gate (audit 1,2,16,11,10,37)   ████████                      ~2–3 wk   BLOCKING
M0       truth in copy                  ▌                             ~0.5 d    parallel, week 1
─────────────────────────────────────────────────────────────────────────────────────────
Phase 1  M1 notifications + digest          ████████                  ~2 wk
         M2 spend caps                          ██████                ~1.5 wk   after M1
         M3 mobile approvals                        ██████            ~1.5 wk   after M1
Phase 2  M4 value ledger / scorecard                   ████████       ~2 wk     needs 11,16
Phase 3  M5 activation + base agents        ████████                  ~2 wk     parallel-able
         M6 import/export/webhooks                        ██████      ~1.5 wk   after M1
Phase 4  M7 ⌘K search                                        ██████   ~1.5 wk
         M8 cycles + recurring                               ██████   ~1.5 wk
         M9 guest access                                       ████   ~1 wk
Phase 5  M10 support workbench                                  ████████████    L — design first
         M11 generic OIDC                                       ████            S–M
```

**The critical path is M1.** Four later milestones depend on it — M2's breach alerts, M3's
deep-links, M6's webhook dispatcher, and M8's recurring materialization all reuse either its
scheduler or its fan-out point. It's also the milestone that most immediately improves a product
that's already shipped. Start there.

---

## What this plan deliberately does not do

Worth stating, so these read as decisions rather than oversights:

- **Make the whole app responsive.** M3 builds one narrow mobile surface. Seven responsive work
  surfaces is an L-sized project with a poor return against a desk-first user base.
- **Build SAML or SCIM.** M11 stops at OIDC. Revisit when a named customer asks.
- **Build marketing and sales workbenches.** M10 takes one role. Three at once is how the whole
  phase slips.
- **Ship multitenancy.** M9's guest access solves most of what agencies need at a fraction of the
  cost. Multitenancy stays roadmapped.
- **Re-plan the engineering audit.** Phase 0 references its task list rather than restating it.

## Risks

- **Phase 0 slips and everything slides.** The migration fix (audit tasks 1–2) is genuinely small;
  per-agent credentials (task 10) landed for the API surface, but the L that actually gates M2's and
  M4's per-agent scope — per-agent **gateway** keys — is still unbuilt and is separate work.
  Mitigation: ship both at org/key scope first, as noted — neither milestone blocks on per-agent
  metering to be valuable.
- **M4 built on unfixed metering.** The failure mode is a plausible dashboard that's wrong, and it's
  tempting because the surface work is easy and the fixes are elsewhere. Enforce the dependency:
  no scorecard merge until `taskId` flows and the HITL invariant is in `updateTask`.
- **M1's scheduler changes existing behavior.** Moving comms decay and outreach off opportunistic
  kicks onto a real timer means they'll start running on idle instances — correctly, but for the
  first time. Expect a burst of decay and outreach activity on the first deploy against any instance
  that's been quiet. Worth a one-time flag.
- **M10 is a different product.** A support workbench has its own intake, its own work-item shape,
  and its own quality model. If it's approached as "the dev workbench with different tools," it will
  cost twice the estimate. The design document is not optional.
