# Talaria — product gap audit

_2026-07-31 · product-management lens · against `1ff47b0`_

Companion to [`AUDIT-2026-07-30.md`](./AUDIT-2026-07-30.md), which covers engineering quality. This
one asks a different question: **given what Talaria promises, what is missing from the feature set?**
Nothing here is a bug. Every gap below is a thing that was never built.

Claims marked **[verified]** were checked against source; the rest is product judgment.

---

## Verdict

The surface coverage is genuinely unusual for a project this age — seven work surfaces, an app
platform, an MCP governance plane, a sandboxed execution layer. The breadth is not the problem.

The gaps cluster in one place, and it's a coherent place:

> **Talaria is built for the moment work happens, and thin everywhere around it.**

Creating work, dispatching it, and watching an agent do it are all first-class. What's missing is
everything on either side of that moment: getting a team *onto* the platform, knowing what happened
*while you were away*, proving *what it was worth*, and controlling *what it's allowed to spend*.

That matters more than usual here, because the pitch isn't "a better board." It's *"manage an AI
workforce."* Management is mostly the surrounding activity: hiring, reviewing, budgeting, reporting.
Talaria currently ships the doing and not the managing.

Three claims in `README.md` / `docs/PRODUCT.md` currently outrun the code. Worth fixing the copy or
the code, but not leaving as-is:

| Claim | Reality |
|---|---|
| "you know exactly what your AI workforce costs **and what it shipped**" (README:35) | Cost side ships. Shipped side does not exist. [verified] |
| PRICING.md sells SSO at the Business tier | Auth is password + Google OAuth only. [verified] |
| PRICING.md sells "daily backups" at Starter | Backup + verified restore now ship (`bun talaria backup` / `bun talaria restore`, `docs/BACKUPS.md`). Still not *daily*: nothing schedules the command, and the documented restore drill has never been run. [verified] |

---

## G1 — The ROI story is half-built: there is no value ledger

**What's missing.** Talaria measures spend in fine detail and measures output not at all.

**Evidence [verified].** `costOverview()` (`usage.ts:184`) returns totals, per-model, per-agent, and
per-day — all tokens and dollars. There is no throughput, cycle-time, completion, or quality metric
anywhere in `server/`. `judge_reviews` rows are written by `judge.ts:154` and read back in exactly
two places, both per-ticket (`judge.ts:48`, `judge.ts:167`) — the verdict history is never
aggregated. Grep for `throughput|velocity|cycle time|lead time` across `server/` returns nothing.

**Why it matters.** This is the single biggest gap in the product, because it undercuts the core
value proposition rather than merely lagging it.

- **The buyer's question is unanswerable.** "Hire another teammate for ~$20/mo" (PRICING.md:39) is a
  *comparison* — against a contractor, a seat, a headcount. A comparison needs both sides. Today an
  admin opening `/observability → Cost` learns that Aria burned $47 last month and learns nothing
  about whether that was a bargain.
- **It's the churn defense.** Month 2 of any AI product is when someone asks "are we actually getting
  anything out of this?" A product that can't answer loses on vibes.
- **It's the management loop.** Agents are framed as staff. Staff get performance reviews. There is
  currently no way to tell a good agent from a bad one except anecdote — no way to know which agent
  keeps getting bounced by the judge, which one's work sails through, which one reports gaps
  constantly because it's mis-scoped.
- **The data is already there.** This is a reporting gap, not an instrumentation gap. `tasks`,
  `task_activity`, `judge_reviews`, `capability gaps`, and `usage_events` already hold everything.

**What to build.**

1. **Agent scorecard** (per agent, per period): tickets carried to review · approved first-pass vs.
   bounced · judge verdict mix (pass/revise/escalate) · mean revisions · dispatch→review cycle time ·
   gaps reported · spend · **cost per approved ticket**. That last number is the one that sells.
2. **Board rollups**: throughput over time, WIP, aging work, human vs. agent split of completed work.
3. **A fleet view of the above** — the "how is my workforce doing" page that `/observability`
   currently gestures at with cost alone.

**Size:** M. Mostly SQL over tables that exist, plus one new surface.

---

## G2 — Human-in-the-loop only works if the human is looking at the tab

**What's missing.** Notification delivery. Everything routes to an in-app inbox and stops there.

**Evidence [verified].** `sendEmail()` in `server/email.ts` is imported by exactly one module:
`invites.ts:8`. Nothing else in the codebase sends mail. `server/notifications.ts:1` says it plainly
— *"Notifications — a user's inbox. Mentions today; more kinds later."* Rows are written to a
`notifications` table and rendered in-app. No email digest, no push, no webhook, no SMS.

**Why it matters.** Talaria's headline guarantee is that a human signs off. But the product is
deliberately asynchronous and increasingly autonomous: agents run on crons, proactive outreach sweeps
run on a timer, work sessions continue turn after turn, the QA judge escalates on its own. All of
that happens whether or not anyone is watching — and *none of it can reach a person who isn't in the
tab.*

The failure mode isn't dramatic, which is what makes it dangerous: it's a slowly filling approval
queue. An agent drafts an email that needs a confirm-send at 11pm and it sits until someone happens
to open Home. A judge escalates a ticket a human never sees. A blocked agent stays blocked all
weekend. The guardrails are all real and all correctly enforced — and they turn into latency, which
is exactly the thing that makes people switch autonomy off.

There's an irony worth naming: `message_user` was built so agents can proactively reach out to
humans (TODO.md:216) — and it delivers into the same tab nobody is looking at.

**What to build.**

1. **Notification routing per user** — in-app / email / (later) push, per event class. Start with
   email; the transport, SMTP-or-Resend with sealed credentials, is already built and used for
   invites.
2. **A daily/period digest** — "3 items need your approval, 2 tickets in QA, 1 agent blocked."
3. **Escalation on pending approvals** — an approval that ages past N hours nags, then escalates to
   an admin. This is what converts the HITL gate from a bottleneck into a genuine SLA.
4. **Reframe the roadmapped Slack/Matrix connectors** (ROADMAP:110) as *HITL delivery channels*, not
   just outbound notification. That's a meaningfully more valuable framing of the same work: approve
   an agent's PR from Slack.

**Size:** M for 1–3, and it substantially de-risks every autonomy feature already shipped.

---

## G3 — "See the money" is observation with no control

**What's missing.** Budgets, caps, and forecasts.

**Evidence [verified].** No `budget`, `quota`, `spend_limit`, or cap concept in `llm-gateway.ts`,
`gateway.ts`, or `usage.ts`. The eng audit reached the same conclusion from the reliability side
(AUDIT-2026-07-30 P1: "No spend cap, quota, rate limit, or circuit breaker exists anywhere in the LLM
path").

**Why it matters — the product framing, distinct from the engineering one.** A spend cap is not
primarily a safety feature here. It's the feature that makes someone comfortable turning autonomy
**on**.

The product asks a buyer to let software with a shell, a git token, and a cron schedule spend their
money unsupervised. The honest answer to "what's the worst case?" is currently "unbounded." Every
serious buyer asks this in the first meeting, and "we meter it carefully after the fact" is not an
answer — a ledger is a receipt, not a brake.

There's a self-serve business consequence too: for the managed cloud, per-org and per-agent caps
aren't optional. They're how the inference allowance in every tier of PRICING.md is enforced. The
pricing model currently has no enforcement mechanism.

**What to build.**

1. **Per-org and per-agent monthly budgets**, checked before the upstream call.
2. **Threshold alerts** at 50/80/100% into the existing `/alerts` plane.
3. **Behavior on breach, configurable**: warn · pause non-interactive work (crons, outreach,
   workbench) but keep human-facing chat alive · hard stop. The middle option is the good default and
   the one that demos well.
4. **A simple forecast** on `/observability` — run-rate against budget. Cheap, given `perDay`.

**Size:** M. Mostly one check in the gateway plus a settings surface.

---

## G4 — Nothing gets in, and nothing gets out

**What's missing.** Importers, exporters, webhooks, and a human-facing API.

**Evidence [verified].** No Jira / Linear / Asana / Trello / CSV import path anywhere. No CSV or
export endpoint for boards, tickets, comms, or the ledger (artifact *sheets* round-trip CSV in-app,
and artifact files download — that's the extent of it). No webhooks. Programmatic access is the fleet
key for agents and the MCP server; there is no per-user API token. The MCP-out connector that would
let external tools reach in is roadmapped, not built (ROADMAP:110).

**Why it matters.**

- **Import is the adoption tax.** Every team Talaria is targeting already has a backlog somewhere.
  The current ask is "retype it." That's a real reason a trial dies in week one, and it's the reason
  every PM tool ships importers early — they're cheap and they remove the largest single objection.
- **Export is a *positioning* asset here, not a feature.** PRODUCT.md:57 says the cloud "sells
  convenience, not captivity." That's a strong line and it's currently unbacked: there's no way to
  get your data out. For an MIT-licensed, self-hostable product whose entire trust position is
  anti-lock-in, shipping a "download everything" button is disproportionately valuable relative to
  its cost. It converts a slogan into a demo.
- **Webhooks are the cheap integration story.** They cover the long tail — Zapier, n8n, internal
  tooling, an ops channel — for a fraction of the cost of building connectors one by one, and they
  buy time on the connectors roadmap.

**What to build.** CSV/JSON import for tickets (with a column mapper) → a Linear/Jira importer →
full-org JSON export → outbound webhooks on ticket/approval/alert events → per-user API tokens.

**Size:** S each, M for the set. Unusually high value-per-hour.

---

## G5 — Approving work is a phone task, and there's no phone

**What's missing.** Any mobile surface.

**Evidence [verified].** 43 responsive Tailwind utilities across the entire `components/` +
`routes/` tree. The nav rail is a fixed `w-56` with no mobile collapse and no `md:hidden` /
`hidden md:` pattern anywhere in `components/app/`. No PWA manifest, no service worker.

**Why it matters.** Look at what the product actually asks a manager to do, many times a day: *look
at a thing an agent did, and say yes or no.* Approve a confirm-send. Clear a QA gate. Unblock an
agent. Ratify a Muse-drafted skill. These are five-second decisions on a small amount of text — the
single most phone-shaped interaction pattern in software.

Right now every one of them requires being at a desk. Combined with G2 (nothing reaches you when
you're away), the practical result is that agent throughput is capped by their manager's desk time.
That's a direct contradiction of the value proposition: agents that work around the clock,
bottlenecked on a human who works at a laptop.

**What to build.** Not the whole app — that would be a large, low-return project. Build a
**mobile approvals surface**: pending approvals, QA gate, blocked agents, mentions, and enough of the
inbox to act. Responsive, installable, notification-linked. This pairs with G2 and should ship after
it.

**Size:** M for the focused surface. L if anyone tries to make all seven work surfaces responsive —
don't.

---

## G6 — Boards have no time dimension, and the app has no search

Two separate gaps that both make the PM suite feel less complete than it is.

**Time [verified].** No cycles, sprints, or iterations. No milestones or releases. No recurring
tickets. Gantt, saved views, custom statuses, dependencies, and estimates all ship — so the
sophistication is clearly there — but there's no way to express *"this batch of work, this period."*

Cycles are the organizing unit of both Linear and Plane, and "Plane/Linear-grade" is a claim the
README makes directly. They're also the natural container for the G1 reporting: a scorecard is much
more legible as "this cycle" than as a rolling 30 days.

Recurring work deserves a specific note, because it's more valuable here than in a human PM tool:
weekly reports, monthly reconciliations, standing checks are *exactly* what you want to hand an
agent. Per-agent Hermes crons exist, but they're agent-side — recurring work has no ticket, no board
presence, no template, no QA gate, and no audit trail. A recurring *ticket* would route through the
whole governed lifecycle the platform already built.

**Search [verified].** There is no global search and no command palette. `kb.search` covers KB docs;
`rag.search` serves agents. `server/tasks.ts` has no search function of any kind — no `ilike`, no
`tsquery` — so tickets cannot be found by title, on one board or across boards. (This was already
noticed from the agent side and deferred: TODO.md:175 lists "ticket search by title (cross-board)"
as a known MCP toolkit gap.)

The inversion is worth stating plainly: **the agents have better retrieval over this workspace than
the humans do.** Agents get hybrid dense+sparse RRF search across four brains. A human gets a
per-surface filter box and their browser history. With seven content-producing surfaces, that's the
kind of gap that quietly makes a product feel smaller than it is.

**What to build.** Cycles → recurring tickets → milestones. Separately: cross-surface search behind
`⌘K`, over tickets, docs, artifacts, plans, messages, and people/agents. The retrieval plane is
already built and indexing most of this; it mostly needs a human-facing front door and ACL-correct
result filtering.

**Size:** Cycles M · recurring M · `⌘K` search M.

---

## G7 — Only one role can actually execute

**What's missing.** Workbench profiles for non-engineering roles.

**Evidence.** The Workbench is dev-first by design and dev-only in fact: GitHub, branches, PRs,
coding harnesses (`WORKBENCH.md`). Marketing, sales, and support are planned as separate installable
apps (README, ROADMAP:98–99). `apps/` currently contains one reference app, `contacts`.

**Why it matters.** The promise is "run your company on agents." The reality is that engineering
agents *ship work*, and every other agent talks about work — chat, docs, tickets, research. Those are
real, but they're assistance, not execution. The difference is exactly the difference between the
$20/mo "hire a teammate" story and a copilot subscription.

This is the largest TAM-shaped gap in the product. It's also the hardest, which is why it's worth
being deliberate about sequencing rather than doing all three at once.

**What to build.** One second role, chosen for measurability rather than market size. **Support** is
the strongest candidate: high volume, bounded actions, an obvious quality gate that maps onto the
existing judge, and outcomes that are trivially countable — which feeds G1 directly and gives the ROI
story its best case study. Marketing/content is the runner-up (easy to demo, harder to score).

Related and much cheaper: **role-ready base agents** are still planned (ROADMAP:108). A starter
roster is a days-not-weeks project that compresses time-to-value sharply — see G8.

**Size:** L for a second workbench role. S–M for base agents.

---

## G8 — There is no activation path

**What's missing.** A first-run experience.

**Evidence.** README's quick start is: run `setup.sh` → sign in → add an LLM provider on `/models` →
set the organization in Admin → design an agent on `/agents`. Four configuration steps before any
value. There's no first-run checklist, no seeded sample board, no demo content, no guided path, and a
hard dependency in the middle: **without a provider key, nothing in the product does anything.**
There's an assistant wizard (`components/assistant`) and an agent import wizard, but no org-level
onboarding.

**Why it matters.** The funnel is unattended by design — MIT, self-hosted, a stranger cloning a repo
at 11pm. Nobody is on a call walking them through it. Whatever happens in the first ten minutes is
the entire conversion event, and right now those ten minutes are configuration with no payoff. The
most impressive things Talaria does — mention an agent and watch it reply, dispatch a ticket and
watch it get worked — are all gated behind setup a new user has no reason to trust yet.

**What to build.**

1. **A first-run checklist on Home** with real progress: provider → org → first agent → first ticket
   dispatched. It doubles as the activation metric, which currently doesn't exist.
2. **A seeded demo board + sample tickets** on a fresh instance, one click to remove.
3. **Ship the base-agent roster** (G7) so "hire your first agent" is a pick, not an authoring task.
4. **Make the payoff the first step, not the last.** The fastest possible path from `dev.sh` to
   "an agent replied to me" is the whole game.

**Size:** M. Highest leverage per hour of anything in this document for the OSS funnel.

---

## G9 — Agencies are a named target segment with an unaddressed core need

**What's missing.** Guest / external collaborator access.

**Evidence.** Access is binary: org member, or fully public via `public_slug`. There is no guest
role, no per-item external invite, no client-scoped view. Multitenancy is roadmapped (ROADMAP:113).
PRODUCT.md:65 names "agencies" in the beachhead segment explicitly.

**Why it matters.** An agency's work is organized per client, and clients need to see a board, review
a deliverable, or comment on a plan — without a seat, and without seeing the other eleven clients.
Right now the only options are "make them a full member of your org" or "publish it to the internet."
Neither is acceptable, so the named segment can't run its actual business model on the product.

The good news: the substrate exists. KB/artifact sharing already has viewer/editor grants, and the
permission plane already does per-person view gating. A guest is mostly a role that composes those
with a member-list exclusion.

**What to build.** A guest role: no seat, invited per board / plan / doc / artifact, sees only what
they're granted, can't see the member directory or any other surface.

**Size:** M. Lower than it sounds, given the existing grant model.

---

## G10 — The "Next" segment's gates are unbuilt (and partly pre-sold)

**What's missing.** SSO/SAML/OIDC, SCIM, MFA, retention policy, legal hold.

**Evidence [verified].** `server/auth/` contains `password.ts` and `google.ts`. That's the whole
provider set. No generic OIDC, no SAML, no SCIM, no MFA anywhere. PRICING.md:35 lists SSO and
"compliance retention" as Business-tier features.

**Why it matters — and why it's last.** For the stated beachhead (1–50 technical people), this is
correctly deprioritized, and I wouldn't move it up. But PRODUCT.md:66 names the growth path as "the
same companies as they grow: permissions, SSO, compliance retention, multitenancy," and PRICING.md
already sells two of those at a specific price. The gap between the pricing page and the codebase
should be closed in the pricing page for now, and in the codebase before the first Business-tier
conversation.

Generic OIDC is the cheap 80% — it covers Okta, Entra, Auth0, and Keycloak in one adapter, and the
auth provider registry is already pluggable and env-gated. SAML and SCIM can wait for a customer who
names them.

**Size:** OIDC S–M. SCIM M. Retention M.

---

## Sequencing

Ordered by value ÷ effort, which is not the same as ordered by importance.

**Now — the promise doesn't land without these**

| | Gap | Why now | Size |
|---|---|---|---|
| 1 | **Notification delivery + digest** (G2) | Every autonomy feature already shipped is throttled by this | M |
| 2 | **Agent scorecard / value ledger** (G1) | Answers the buyer's and the renewer's question; data already exists | M |
| 3 | **Spend caps + budgets** (G3) | The feature that lets people turn autonomy on | M |

**Next — adoption**

| | Gap | Why | Size |
|---|---|---|---|
| 4 | **First-run activation + base agents** (G8, G7-partial) | Highest leverage per hour for the OSS funnel | M |
| 5 | **Import / export / webhooks** (G4) | Removes the largest trial objection; backs the anti-lock-in position | S–M |
| 6 | **Mobile approvals surface** (G5) | Unblocks the desk-time ceiling; pairs with #1 | M |

**Then — surface completeness**

| | Gap | Why | Size |
|---|---|---|---|
| 7 | **`⌘K` cross-surface search** (G6) | Seven content surfaces, no way to find anything | M |
| 8 | **Cycles + recurring tickets** (G6) | Table stakes for the claim; recurring is agent-shaped work | M |
| 9 | **Guest access** (G9) | Unblocks a named beachhead segment | M |

**Bets — pick deliberately, not all at once**

| | Gap | Why | Size |
|---|---|---|---|
| 10 | **Second workbench role (support)** (G7) | Largest TAM expansion; also the best ROI case study | L |
| 11 | **Generic OIDC** (G10) | Cheapest 80% of the enterprise gate; close the pricing-page gap first | S–M |

**Also, cheaply: fix the three overstated claims** in the table at the top. Either build the thing or
soften the sentence — an audit that finds the marketing ahead of the code is a much smaller problem
when the fix is a paragraph.

---

## What this audit deliberately does not cover

Engineering quality, security, correctness, and process — all covered in
[`AUDIT-2026-07-30.md`](./AUDIT-2026-07-30.md), which should be read first and largely acted on
first. Several items there are P0-broken (fresh installs fail; the HITL guarantee has holes;
per-agent identity is forgeable). **None of the gaps in this document are worth starting before
those are closed** — a feature set is only as good as the platform it sits on, and a product gap is a
much better problem to have than a broken install.
