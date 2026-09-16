# Talaria apps — self-contained apps within the app

Apps extend Talaria with whole new surfaces — a CRM, a support desk, a marketing planner — as
**self-contained TypeScript codebases this instance compiles and runs** without a host rebuild.
Not iframes, not webhooks: an app's views live in the same router, design system, and session as
core surfaces. Authors write TypeScript against `@talaria/sdk` — never Rust.

Three parties build them: Outcrop (official apps, maintained outside core), your own developers,
and the community. Core ships with none enabled.


## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces
  app.ts         UI surfaces (Svelte 5) — default-exports defineApp({...})
  *.svelte       one component per surface (plus whatever else you need)
  server.ts      optional API — defineAppServer(...) → /api/apps/<slug>/*
  mcp.ts         optional agent tools — defineAppMcp(...) → governed MCP server
  harness.ts     optional workbench harness — defineWorkbenchHarness({...})
  harnesses/     optional activity harnesses — one defineHarness per file
```

The programming model — `defineApp`, the app server, the document store, MCP tools, harnesses —
is the [SDK docset](./sdk/README.md). Scaffold with `bun talaria app new <slug>`; the worked
example is [`apps/contacts`](../apps/) in the repo.


## Lifecycle

1. **Install** — the app's codebase lands in `apps/`:
   - from the **marketplace** (Manage → Apps → Discover): one click shallow-clones the repo;
   - from **any https git URL** with a `talaria.json` at its root;
   - or by hand: drop/clone a directory into `apps/`.
2. **Build** — this instance compiles the app against the host runtime (`/runtime/rt-*.js`)
   and starts a dedicated Postgres container for its data. Dev loads sources through Vite
   (HMR); production serves the compiled chunk. A failed build stays on that app.
3. **Enable** — Manage → Apps (admin). Enable compiles and loads the app first
   (UI surfaces, `server.ts` `fetch`, `mcp.ts` `tools`); a broken build is refused
   with that error. An already-enabled app that later fails is turned off by the
   boot reconciler — opening the Apps page is a read.



4. **Grant** — apps are **explicit-grant**: enabling one gives members nothing. Admins allow each
   app view per person in Admin → People — the same checklist as core Manage views, enforced
   server-side at the app API gateway.
5. **Uninstall** — removes the codebase, stops its database container, and (optionally) wipes
   stored data. Audited, like enable/disable/install.


## The marketplace

Manage → Apps → **Discover** reads a catalog index of community + official apps (configurable
source URL; defaults to Outcrop's registry). Official apps are maintained by Outcrop Labs and
labeled as such; community apps list their author and repo. The catalog being unreachable degrades
gracefully — install-from-git always works.

**Trust:** installing an app adds code to your deployment that runs fully trusted, like the
platform itself. Install only apps you trust; the UI says this wherever code enters the system.
The runtime safety boundary is the user session — see the
[security model](./sdk/README.md#security-model).

## Publishing an app

1. Push the app to a public https git repository with `talaria.json` at the root. Repo naming
   convention: `talaria-app-<slug>` (the prefix strips on install).
2. Anyone can now install it from its git URL.
3. To appear in the marketplace, submit it to the catalog index (`outcrop-labs/talaria-apps`).

Version bumps ship as git commits; an instance updates by re-pulling the app directory and
rebuilding (managed update flows are on the roadmap).

## Operational notes

- `TALARIA_APPS_DIR` overrides where app codebases live (default: `apps/` beside `ui/`).
- `TALARIA_APP_BUILDS_DIR` holds compiled chunks; `TALARIA_APP_DATA_DIR` holds each app's
  Postgres volume, compose file, and `db.env`. Both default beside the apps dir. Uninstall deletes them.
- Each app gets its own Postgres (a compose project, image digest-pinned when the
  local image has one). The password is sealed in `app_settings` and written to a
  0600 `db.env` — never on `docker run -e` and never derived from `TALARIA_SECRET_KEY`.
  `talaria backup` dumps each app DB into the snapshot. The document store (`ctx.store`)
  is the API; app authors never see a connection string.

- A throw in an app's UI, server, or MCP is isolated: the pane shows the crash, the rest of
  Talaria keeps running.
- App MCP servers appear in Manage → MCP badged **app**; their tool catalogs come from the module
  (no probe), and they cannot be deleted or re-pointed there — lifecycle belongs to Manage → Apps.
- Everything an app's users do runs under their own session: platform permissions, resource ACLs,
  and the audit log apply exactly as in core surfaces.


## Shipping a harness

Talaria has **two** things called a harness and they are not specializations of each other:

- a **workbench harness** (`apps/<slug>/harness.ts`) — a coding CLI agents drive in the sandbox:
  [workbench-harnesses.md](./sdk/workbench-harnesses.md), deep contract in
  [WORKBENCH.md](./WORKBENCH.md);
- an **activity harness** (`apps/<slug>/harnesses/*.ts`) — a model call Talaria runs on your app's
  behalf, with evals that earn a column in the org's model-fitness matrix:
  [harnesses.md](./sdk/harnesses.md), deep contract in [HARNESSES.md](./HARNESSES.md).

The host merges definitions from three layers by slug/id (later wins): Talaria's builtins ←
app-shipped (enabled apps only) ← admin-registered custom JSON (`PUT /api/workbench/harnesses`,
`agents.manage`). A registered harness plugs into the whole workbench machinery automatically —
selectable per agent, auth and MCP grants provisioned at render time, probe surfaced by the
workbench doctor — and jobs, branches, PRs, plan gates, per-job workspaces, and shared session
history all behave identically: the harness is just the tool inside the flow.
