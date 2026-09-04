# API reference — system

> **Generated** by `bun run docs:api` from the Rust router table (`api/src/routes/mod.rs`)
> and the handler modules under `api/src/routes/**` (the TS residents still serving
> `healthz` and the app dispatch excepted) — do not edit by hand.
> Change the route (or its `// doc:` note) and regenerate; `bun run check` fails on drift.
> The **Returns** column is the first success-shaped `json!({…})` literal and is heuristic —
> `…` means the shape is not a literal in source.

2 routes.

| Route | Method | Auth |
| :--- | :--- | :--- |
| [`/api/healthz`](#apihealthz) | GET | `public` |
| [`/api/well-known/talaria-instance`](#apiwell-knowntalaria-instance) | GET | `public` |

## `/api/healthz`

Source: [`ui/src/routes/api/healthz.ts`](../../ui/src/routes/api/healthz.ts)

> Liveness/readiness — SQL and Redis round-trips, plus a `migrations`
> check that appears (and fails the probe) when the boot migration pass
> died, and `version` (the image's TALARIA_VERSION, null on local
> builds) so a deploy gate can tell two instances apart. PUBLIC BY
> DESIGN: no session guard, because a health check that needs a session
> tells you nothing exactly when you need it.

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{status, version, uptimeSeconds, checks}` | 200 + varies | — |

## `/api/well-known/talaria-instance`

Source: [`api/src/routes/system/well_known_talaria_instance.rs`](../../api/src/routes/system/well_known_talaria_instance.rs)

> /api/well-known/talaria-instance.
>
> Instance identity beacon — the target of hosting-domain verification's
> self-fetch, and the SPA's one boot-time read of the instance's display
> …

| Method | Auth | Body | Returns | Status | Flags |
| :--- | :--- | :--- | :--- | :--- | :--- |
| GET | `public` | — | `{instance, companyName}` | 200 | — |

