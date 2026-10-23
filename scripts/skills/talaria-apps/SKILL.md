---
name: talaria-apps
description: Build Talaria apps — TypeScript against @talaria/sdk, never Rust. Anatomy, store, MCP tools, UI kit, and the hard rules so an app cannot take down the host.
---

# Talaria apps

When to use: you are creating or changing a Talaria app (a `talaria-app-<slug>` repo, `@talaria/sdk`). This is the playbook. The programming model is the SDK docset; this skill is what not to get wrong.

Talaria apps are **TypeScript** (Svelte 5 runes) that this instance compiles. They render as native platform surfaces — same router, design system, and session. Not iframes, not webhooks, not a second backend. Authors never write Rust.

## Start

An app is a **git repository**, built like any other code you ship: as a workbench job. There is no Talaria checkout in your container, no `apps/` directory to write into, and no `bun talaria` — those belong to the host an admin runs. The path is:

1. Get a ticket for it and `start_job` on the app's repo — a `talaria-app-<slug>` repository (the `talaria-app-` prefix strips on install; the slug is lowercase kebab, `^[a-z0-9][a-z0-9-]{0,63}$`, and not `layout`, `hooks` or `meta`, which install refuses). A new app starts as a new repo a human creates and grants you; the first job scaffolds it. Drive the harness (**workbench-driving**, **oh-my-pi**); never hand-code.
2. The anatomy at the repo root: `talaria.json`, `app.ts`, `Work.svelte`, `server.ts`, `mcp.ts` (the last three optional). Build it in the job from the shapes in [references/sdk.md](references/sdk.md).
3. `finish_job` → the pull request → a human merges. **An admin installs it** from the repo URL (Manage → Apps → install from git, or the marketplace once it is listed), enables it in **Manage → Apps**, and grants views per person in **Admin → People**. You cannot install, enable or grant — put the URL in your outcome and say what to enable.
4. Read [references/sdk.md](references/sdk.md) before inventing imports.

## The reflexes

**Import only from the SDK.** `@talaria/sdk` (client: kit, session, `api` / `appApi` / `useAppQuery`) and `@talaria/sdk/server` (server, store, MCP). Plus `svelte` and `@lucide/svelte`. Never `ui/src`, never a relative reach into the host, never a raw provider URL.

**The host already did the trust work.** `server.ts` runs only after session, enablement, and view grant. `ctx.user` is authenticated. `ctx.store` is *this* app's Postgres — not Talaria's catalog. Never open `DATABASE_URL`. Never `process.exit`.

**Surfaces fill the pane.** App views are not `PageSurface` (that's host chrome). Match the skeleton: `h-full overflow-y-auto p-8` and a centred column (`max-w-3xl`). Kit components only — Button, EmptyState, Input, Chip, SkeletonRows, confirm(). Destructive actions are quiet (`DangerLink` / confirm), never a giant red button.


**Platform data goes through `api()` on the client.** Boards, tickets, agents — `/api/…` as the signed-in user. Every ACL applies. An app cannot do more than the person using it.

**Agents get `mcp.ts`, not a side channel.** Prefix tool names (`pulse_list`, not `list`). Return small JSON. Throws become the message the model sees. Governance is Manage → MCP; you do not opt out, and every call — refused ones too — is recorded (never its arguments). **A tool argument may arrive carrying a resolved credential**: an agent passes a `«secret:…»` handle and the gateway resolves it on the way in, so `args` can hold a real key. Use it for the call and let it go — never `ctx.store.insert(…, args)`, never log arguments, never echo them in a thrown message. Your `inputSchema` is enforced before the handler runs; a bad shape never reaches you.

## Hard rules

- TypeScript only. No Rust in an app. No compiling the app into the host bundle — the instance builds it.
- No `import.meta.glob`, no host rebuild, no "awaiting build" as a reason to edit `ui/`.
- No secrets in the app tree. Keys stay in Talaria's sealed store; the app uses the session.
- Do not force a ticket `done`. Do not assign work to others.
- A throw in your UI, server, or MCP must stay in the app. Do not catch-and-ignore; let it fail so the pane can show the crash. The isolation covers **awaited** throws only: a detached `setTimeout(() => { throw })` or an un-awaited rejecting promise is the host's unhandled rejection, and `process.exit` or an infinite loop is the host's too. Await what you start; do not take the process down.
- Do not add `package.json` unless this is a gitignored subrepo with its own node_modules (those are excluded from the host typecheck on purpose).

## Quick map

| You need | Reach for |
|---|---|
| New app | a `talaria-app-<slug>` repo, built as a workbench job; an admin installs it from the URL |
| UI kit / motion | `@talaria/sdk` — Button, EmptyState, Input, Chip, `fade`/`QUICK` |
| Session | `useMe`, `useIsAdmin`, `useHasPerm` |
| Your API | `appApi(slug)`, `useAppQuery`, `useAppInvalidate` |
| Platform API | `api('/api/…')` as the signed-in user |
| Document store | `ctx.store` in `server.ts` / `mcp.ts` (`list/get/insert/update/remove`) |
| Agent tools | `defineAppMcp` in `mcp.ts` |
| Body validation | `parseBody(request, z.object({…}))` — 400 on failure |
| Full docset | `/opt/skills` does not ship the SDK docs; the shapes are in [references/sdk.md](references/sdk.md) |

Grow from the skeleton, not by inventing a second stack.

