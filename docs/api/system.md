# API reference — system

> **Generated** by `bun run docs:api` from `ui/src/routes/api/**` — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
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

Source: [`ui/src/routes/api/well-known.talaria-instance.ts`](../../ui/src/routes/api/well-known.talaria-instance.ts)

> Instance identity beacon — the target of hosting-domain verification's
> self-fetch. Public and harmless: a random UUID that proves which
> deployment answered, nothing more.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{instance}` | 200 | — |

