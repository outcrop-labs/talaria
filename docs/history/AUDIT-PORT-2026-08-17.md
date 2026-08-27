# Audit fixes — ported to `main` (2026-08-17)

The `wt/audit-fixes` branch (18 commits, cut 2026-07-30) was never merged. It predates two main-line
rewrites — the React→Svelte migration and the harness port — so a direct merge produced **217
conflict hunks across 117 files**. Instead of merging, each finding was checked against current
`main` and only the live ones were ported.

`main` at triage time: `62196fb`. Source branch: `wt/audit-fixes` @ `1e30cc7` (preserved, not
deleted — see "What is left" before retiring it).

## The port already in progress

This was not a fresh start. `0f3b2b5` — **"Port the audit branch's tests, and fix the five bugs they
found" (#209, 2026-08-05)** — already took the branch's test harness and tests onto `main`, and
found five real defects doing it (`parseAgentStream` reader lock, the secret-leak guardrail
mislabelling `sk-ant-` keys, `ChatEvent` missing `queued`, `localDateAtHour` accepting `2026-13-45`,
`initSecretbox` not clearing a stale failure). `main` went from 5 test files to 82.

That commit named what it was leaving behind, and set the condition for retiring the branch:

> `safe-fetch.ts` (SSRF) has 7 call sites on the branch, all in modules main has since rewritten,
> and a guard that blocks localhost would break self-hosted local LLM endpoints — a first-class use
> case here. It needs its own change with its own care, not an unwired module landed on a promise.
> `rate-limit.ts`, `env.ts` and `actor.ts` are in the same position.
>
> `wt/audit-fixes` stays until those land.

Eighty commits later, none had. **They have now** — three landed, one was superseded.

### Two corrections to #209's reasoning

1. **"A guard that blocks localhost would break self-hosted local LLM endpoints" — the module never
   proposed to.** `safe-fetch.ts` exempts operator-configured first-party infrastructure (Qdrant,
   embedder, object storage, the loopback toolkit, and **admin-registered LLM endpoints**), because
   those URLs come from the operator rather than from a user or an agent. `llm-gateway.ts` says so
   at its call site and keeps its bare `fetch`. `TALARIA_FETCH_ALLOW_HOSTS` is the escape hatch.

2. **"All in modules main has since rewritten" — two of seven.** Since the split at #201, `main`
   left `apps.ts`, `mcp-library.ts`, `mcp-oauth.ts` and `mcp-probe.ts` untouched, so the branch's
   versions applied exactly. Only `mcp-registry.ts` needed reconciliation; `llm-gateway.ts` is
   heavily rewritten but is deliberately outside the guard.

## What landed

| Commit | Findings |
|---|---|
| `b35b8af` | **S13** SSRF — `safe-fetch.ts` + 7 call sites (MCP probe/registry, six OAuth hops, app catalog, favicon proxy) |
| `863378c` | **S11** Redis-backed login limiter (per-username + per-IP); `env.ts` boot validation |
| `59a3f6e` | **S1** KB-search stored XSS · **S3** OAuth-callback reflected XSS + CSP · **S9** `moveDoc` and doc-create IDORs · **S2** workbench mounts/image admin-gated + validated · **S14** profile `env` values hidden from non-managers |
| `b6b9837` | **S7** dev ports on loopback · **S6** container `cap_drop`/`no-new-privileges`/pids/mem/cpu ceilings |
| `aba4232` | **S8** retrieval ACL on every collection; effective visibility on every sync path |
| `5b66065` | **S4** GitHub installation tokens scoped to one repo |
| *(this)* | dependency advisory (`nanoid`) cleared |

New tests: 110 (safe-fetch) + 19 (env) + 9 (snippet escaper) + 18 (mount validator). Suite went
1836 → 1992, all green; `svelte-check` 0 errors; `check-invariants` clean.

## Already fixed on `main` — dropped from the port

These landed independently over the 95 commits since the branch was cut. `git cherry` still reported
them as unique patches because they were re-implemented, not cherry-picked.

| Finding | Evidence on `main` |
|---|---|
| P0.1 fresh-install migrations | `schema_migrations` + `pg_advisory_lock` in `server/db/pg.ts` |
| P0.2 agent identity | per-agent `tak_` credentials in `server/agent-auth.ts`; `agentName()` deleted |
| P0.3 human-in-the-loop | `TaskActor` threaded through `updateTask` (`server/tasks.ts:225,509`) |
| S5 fleet secrets world-readable | `mode: 0o600` + `chmod 0700` (`fleet-render.ts:128-139,191`) |
| Tests + CI gate + `/api/healthz` | ported by #209; `.github/workflows/ci.yml` |
| `.env.example` secret-key docs | written since |
| `actor.ts` | `main` has the identical `actorOf` in `api-guard.ts:64`, already widely imported |
| Icon-domain screening | `publicIconDomain` covers more than the branch's `DOMAIN_RE` (localhost, `.local`, `.internal`, IPv6) |
| `internalServiceKey()` for the builtin toolkit probe | `main` has no such thing and still documents `TALARIA_AGENT_KEY` as the app's own hop (`fleet-render.ts:45`) |
| S4's clone-URL leak | `a56f484` stopped `cloneUrl` embedding a live token; only the token *scope* was left |

## Dropped as dead

- `7d3c3ab` **Frontend: let query failures surface, and store one due-date instant** — targets
  `.tsx` files (`field-pills.tsx`, `task-detail.tsx`, `router.tsx`) that no longer exist. The
  underlying complaints (silent query failures, the split due-date write) may still apply to the
  Svelte surfaces, but that is fresh work, not a port.

## What is left — one deliberate carve-out

**`9fbbfc5` "Meter what agents actually spend, and give it a ceiling" was NOT ported.** It is the
only remaining commit, and it is half-superseded:

- **Its attribution half is superseded, and porting it would be a regression.** The branch made
  workbench harnesses authenticate with `TALARIA_AGENT_KEY` so their spend lands on the agent's
  ledger. `main` has since solved the same problem better, with a dedicated `workbench-gateway`
  credential minted into the fleet `.env` as `LLM_WORKBENCH_API_KEY`
  (`fleet-brain.ts:35`, `workbench-harnesses.ts:35-44`). Taking the branch's version would undo it.
- **Its other half is genuinely still open**, and is a real correctness gap rather than a security
  one:
  - **Full token accounting.** `main`'s `usage.ts` has no cache-write / cache-read / reasoning
    token handling at all. Anthropic prices cache writes at 1.25× input and cache reads at 0.1×,
    both separate from `input_tokens`, so the ledger currently *understates* Anthropic spend;
    OpenAI folds cached input into `prompt_tokens` at full price, so it *overstates* there.
  - **Spend ceilings.** No per-org / per-agent rolling budget exists on `main`, and no cron
    frequency floor.

Both need five `usage_events` columns and touch `llm-gateway.ts`, the file the harness port rewrote
most heavily (+357/-45). This is a billing change: getting it wrong mis-invoices in a direction
nobody notices until reconciliation. It wants its own change with its own verification against real
provider responses — which is the same judgment #209 made about `safe-fetch.ts`, and it was right
then.

**Retire `wt/audit-fixes` only after this lands**, or after deciding the budget feature is not
wanted. Everything else on it is now either on `main` or documented above as superseded.
