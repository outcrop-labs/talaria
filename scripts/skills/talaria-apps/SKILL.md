---
name: talaria-apps
description: Build Talaria apps — TypeScript against @talaria/sdk, never Rust. Anatomy, store, MCP tools, UI kit, and the hard rules so an app cannot take down the host.
---

# Talaria apps

When to use: you are creating or changing a Talaria app (`apps/<slug>/`, `@talaria/sdk`). This is the playbook. The programming model is the SDK docset; this skill is what not to get wrong.

Talaria apps are **TypeScript** (Svelte 5 runes) that this instance compiles. They render as native platform surfaces — same router, design system, and session. Not iframes, not webhooks, not a second backend. Authors never write Rust.

## Start

1. Scaffold: `bun talaria app new <slug>` (optional `--name`, `--icon`). Writes `talaria.json`, `app.ts`, `Work.svelte`, `server.ts`, `mcp.ts`. Slug is lowercase kebab, same rule as install (`^[a-z0-9][a-z0-9-]{0,63}$`).
2. If you have no CLI, create that same anatomy by hand under `apps/<slug>/`. Do not copy `apps/contacts` unless you need a CRM — it is the reference, not the starter.
3. Enable in **Manage → Apps**. Members get nothing until an admin grants views in **Admin → People**.
4. Read [references/sdk.md](references/sdk.md) before inventing imports.

## The reflexes

**Import only from the SDK.** `@talaria/sdk` (client: kit, session, `api` / `appApi` / `useAppQuery`) and `@talaria/sdk/server` (server, store, MCP). Plus `svelte` and `@lucide/svelte`. Never `ui/src`, never a relative reach into the host, never a raw provider URL.

**The host already did the trust work.** `server.ts` runs only after session, enablement, and view grant. `ctx.user` is authenticated. `ctx.store` is *this* app's Postgres — not Talaria's catalog. Never open `DATABASE_URL`. Never `process.exit`.

**Surfaces fill the pane.** App views are not `PageSurface` (that's host chrome). Match contacts: `h-full overflow-y-auto p-8` and a centred column (`max-w-3xl`). Kit components only — Button, EmptyState, Input, Chip, SkeletonRows, confirm(). Destructive actions are quiet (`DangerLink` / confirm), never a giant red button.

**Platform data goes through `api()` on the client.** Boards, tickets, agents — `/api/…` as the signed-in user. Every ACL applies. An app cannot do more than the person using it.

**Agents get `mcp.ts`, not a side channel.** Prefix tool names (`pulse_list`, not `list`). Return small JSON. Throws become the message the model sees. Governance is Manage → MCP; you do not opt out.

## Hard rules

- TypeScript only. No Rust in an app. No compiling the app into the host bundle — the instance builds it.
- No `import.meta.glob`, no host rebuild, no "awaiting build" as a reason to edit `ui/`.
- No secrets in the app tree. Keys stay in Talaria's sealed store; the app uses the session.
- Do not force a ticket `done`. Do not assign work to others.
- A throw in your UI, server, or MCP must stay in the app. Do not catch-and-ignore; let it fail so the pane can show the crash. Do not take the process down.
- Do not add `package.json` unless this is a gitignored subrepo with its own node_modules (those are excluded from the host typecheck on purpose).

## Quick map

| You need | Reach for |
|---|---|
| New app | `bun talaria app new <slug>` |
| UI kit / motion | `@talaria/sdk` — Button, EmptyState, Input, Chip, `fade`/`QUICK` |
| Session | `useMe`, `useIsAdmin`, `useHasPerm` |
| Your API | `appApi(slug)`, `useAppQuery`, `useAppInvalidate` |
| Platform API | `api('/api/…')` as the signed-in user |
| Document store | `ctx.store` in `server.ts` / `mcp.ts` (`list/get/insert/update/remove`) |
| Agent tools | `defineAppMcp` in `mcp.ts` |
| Body validation | `parseBody(request, z.object({…}))` — 400 on failure |
| Full docset | `/opt/skills` does not ship the SDK docs; the shapes are in [references/sdk.md](references/sdk.md) |

Contacts (`apps/contacts`) is the worked example: three surfaces, store, MCP. Grow from the skeleton, not by inventing a second stack.
