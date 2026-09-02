# API reference — system

> **Frozen at the cutover (2026-09-01)** — generated from the TS route tree the Rust port
> replaced. Source links point at the Rust modules (`api/src/routes/**`; each module’s
> header names the TS file it ported) or the permanent TS residents still serving.
> Regeneration returns with the Rust extractor (#293); until then, maintained by hand.
> The **Returns** column is the first success-shaped `json({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

2 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/healthz`](#apihealthz) | GET | `public` |
| [`/api/well-known/talaria-instance`](#apiwell-knowntalaria-instance) | GET | `public` |

## `/api/healthz`

Source: [`ui/src/routes/api/healthz.ts`](../../ui/src/routes/api/healthz.ts)

> Liveness/readiness — SQL and Redis round-trips. PUBLIC BY DESIGN: no
> session guard, because a health check that needs a session tells you
> nothing exactly when you need it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{status, uptimeSeconds, checks}` | 200 + varies | — |

## `/api/well-known/talaria-instance`

Source: [`api/src/routes/well_known_talaria_instance.rs`](../../api/src/routes/well_known_talaria_instance.rs)

> Instance identity beacon — the target of hosting-domain verification's
> self-fetch. Public and harmless: a random UUID that proves which
> deployment answered, nothing more.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{instance}` | 200 | — |

