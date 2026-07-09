# Talaria cloud: pricing direction (internal draft)

_Working notes, 2026-07-09. Not public, not final: the thinking behind the tiers so future decisions
have context. Self-hosting is free forever regardless of anything here._

## Principles

1. **Sell agents, not infrastructure.** Every customer gets a dedicated private instance on our own
   hardware, but nobody ever sees a spec sheet. The billable unit is the **org agent** ("hire another
   teammate"), because it's simultaneously the value metaphor, the real capacity driver (each agent is
   a container), and the expansion mechanic. Instance sizing is an implementation detail that follows
   the team size invisibly (the rolling-replacement machinery makes resizes/migrations non-events).
2. **Every human comes with their personal assistant, included.** Personal assistants
   (`agent_defs.owner_user_id` set) are a SEPARATE pool from org agents and never count against the
   agent quota. Counting them would punish exactly the behavior we want ("everyone on your team gets
   their own assistant" is one of the product's best sentences). Human seats therefore carry real
   weight (a container each) and are priced as modest seats.
3. **Value-anchored, not cost-plus.** We host on our own infrastructure (capacity, not cloud rent, is
   the constraint), so prices anchor against what the product replaces: per-seat SaaS ($8–15/human/mo)
   and fractions of headcount. An org agent at ~$20/mo reads absurdly good against both.
4. **Trust the ledger.** The gateway already meters every token per agent/ticket/model, so
   usage-based inference allowances are near-zero build. Bundled allowances served from our own
   inference boxes cost electricity, not API fees: a structural margin competitors reselling API
   tokens can't match. BYO provider keys stay allowed at every tier.
5. **Trial, not a free cloud tier.** Every customer occupies real hardware; a free tier strands
   capacity. 14-day trial (or "first agent free for 30 days") instead. Self-host IS the free tier.

## Draft tiers

| Tier | Price | Org agents | Human seats (each incl. their personal assistant) | Step-up |
|---|---|---|---|---|
| Self-hosted | free forever | ∞ | ∞ | The trust anchor and the funnel |
| Starter | ~$99/mo | 3 | 5 incl., then ~$10/seat | Dedicated private instance, daily backups, BYO keys |
| Team | ~$299/mo | 10 | 20 incl. | Included inference allowance, longer audit retention, priority support |
| Business | ~$799/mo | 25 | 50 incl., volume beyond | SSO, custom domain, compliance retention, guardrail/judge SLAs |
| Enterprise | custom | custom | custom | Multi-instance / multitenancy, or supported deployment on their infra |

**Expansion levers (the whole growth story):**
- **+1 org agent ~$20/mo**: "hire another teammate."
- **+1 human seat ~$10/mo**: "bring another person; their assistant comes with them."
- Extra inference credits / storage as simple add-ons.

## Capacity notes (own-infrastructure reality)

- Model **RAM-per-agent** and set an oversubscription policy before the first ten customers. A
  dedicated instance per customer strands memory if we're not deliberate.
- **Wake-on-message for personal assistants** is the highest-leverage capacity lever: PAs idle most of
  the day; stopping idle PA containers and cold-starting on first message likely doubles+ customers
  per box. The UX substrate already exists: `proxyChat` hold-and-retry bridges a cold start
  invisibly, and `fleetUp` brings the container back.
- **Single-box blast radius**: one host down = those customers down. The slot/port rolling machinery
  is most of the substrate for migrating agents across hosts; multi-host scheduling is the eventual
  answer.

## Open questions

- Annual discount (industry-standard ~2 months free)?
- Does Enterprise "supported self-host" cannibalize cloud, or is it the wedge into orgs that would
  never host with us anyway? (Lean: it's the wedge; self-host support contracts monetize the free
  tier without touching product.)
- Inference allowance sizing: pull real per-agent token averages from the ledger once a few
  customers exist rather than guessing now.
