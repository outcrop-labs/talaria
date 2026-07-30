# Talaria apps — self-contained apps within the app

Apps extend Talaria with whole new surfaces — a CRM, a support desk, a marketing planner — as
**self-contained codebases that compile into the deployment** and render as native platform UI.
Not iframes, not webhooks: an app's views live in the same router, design system, and session as
core surfaces, and its code ships with the deployment ("deploy within the deployment").

Three parties build them: Outcrop (official apps, maintained outside core), your own developers,
and the community. Core ships with none enabled.

## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces
  app.tsx        UI surfaces (React) — default-exports defineApp({...})
  server.ts      optional API — defineAppServer(...) → /api/apps/<slug>/*
  mcp.ts         optional agent tools — defineAppMcp(...) → governed MCP server
  ...            anything else (components, lib, assets)
```

```json
{
  "name": "Contacts",
  "icon": "☏",
  "version": "0.1.0",
  "description": "Lightweight CRM — people, companies, stages, notes.",
  "surfaces": { "work": "Contacts", "manage": "Contacts data", "settings": "Contacts" }
}
```

| surface    | where it appears                       | access                                 |
|------------|-----------------------------------------|----------------------------------------|
| `work`     | Work section of the nav → `/x/<slug>`   | admins; members need an explicit grant |
| `manage`   | Manage section → `/x/<slug>/manage`     | admins; members need an explicit grant |
| `settings` | a Settings tab (for people with access) | follows the work-view grant            |

The programming model — `defineApp`, the app server, the document store, MCP tools — is documented
in [SDK.md](./SDK.md).

## Lifecycle

1. **Install** — the app's codebase lands in `apps/`:
   - from the **marketplace** (Manage → Apps → Discover): one click shallow-clones the repo;
   - from **any https git URL** with a `talaria.json` at its root;
   - or by hand: drop/clone a directory into `apps/`.
2. **Build** — apps compile in with the host. The dev server picks new apps up live; production
   deployments need a rebuild (the Apps view shows cloned-but-not-built apps as **awaiting build**).
3. **Enable** — Manage → Apps (admin). Disabled apps have no nav presence, their API routes 404,
   and their MCP server is retired (carriers get blue/green rolled).
4. **Grant** — apps are **explicit-grant**: enabling one gives members nothing. Admins allow each
   app view per person in Admin → People — the same checklist as core Manage views, enforced
   server-side at the app API gateway.
5. **Uninstall** — removes the codebase and (optionally) wipes the app's stored data. Audited, like
   enable/disable/install.

## The marketplace

Manage → Apps → **Discover** reads a catalog index of community + official apps (configurable
source URL; defaults to Outcrop's registry). Official apps are maintained by Outcrop Labs and
labeled as such; community apps list their author and repo. The catalog being unreachable degrades
gracefully — install-from-git always works.

**Trust:** installing an app adds code to your deployment that runs fully trusted, like the
platform itself. Install only apps you trust; the UI says this wherever code enters the system.
The runtime safety boundary is the user session — see [SDK.md § Security model](./SDK.md).

## Publishing an app

1. Push the app to a public https git repository with `talaria.json` at the root. Repo naming
   convention: `talaria-app-<slug>` (the prefix strips on install).
2. Anyone can now install it from its git URL.
3. To appear in the marketplace, submit it to the catalog index (`outcrop-labs/talaria-apps`).

Version bumps ship as git commits; an instance updates by re-pulling the app directory and
rebuilding (managed update flows are on the roadmap).

## Operational notes

- `TALARIA_APPS_DIR` overrides where app codebases live (default: `apps/` beside `ui/`).
- App data lives in the `app_data` table, namespaced by slug — survives uninstall unless wiped.
- App MCP servers appear in Manage → MCP badged **app**; their tool catalogs come from the module
  (no probe), and they cannot be deleted or re-pointed there — lifecycle belongs to Manage → Apps.
- Everything an app's users do runs under their own session: platform permissions, resource ACLs,
  and the audit log apply exactly as in core surfaces.


## Shipping a harness

Apps can extend the **Workbench** — the sandboxed execution layer Hermes agents drive real work through — by shipping a harness definition:

```
apps/<slug>/harness.ts
```

```ts
import { defineHarness } from '@talaria/sdk'

export default defineHarness({
  slug: 'aider',
  label: 'Aider',
  auth: 'gateway', // OpenAI-compatible → pointed at Talaria's gateway (metered, attributed)
  invoke: 'aider --model <model> --message "<task>"',
  jsonInvoke: 'aider --model <model> --message "<task>" --yes --no-pretty',
  mcpConfig: { format: 'claude-json', filename: 'aider-mcp.json' },
  guide: 'Aider works in git-aware sessions; read its structured output and verify diffs yourself.',
  install: { commands: ['pip install aider-chat'] },
})
```

The definition is **declarative — no host code runs from it**. The host merges harnesses from three layers by slug (later wins): Talaria's builtins (opencode, Claude Code, Codex CLI, Oh My Pi) ← app-shipped (enabled apps only) ← admin-registered custom JSON (`PUT /api/workbench/harnesses`, `agents.manage`).

A registered harness plugs into the whole workbench machinery automatically:

- selectable per agent (the Harness dropdown on the agent's Workbench control), effort→model routing included, with the model rendered in the harness's own syntax (`modelPrefix`);
- `auth: 'gateway'` provisions Talaria's gateway env into the sandbox; `auth: { provider, envVar }` interpolates that provider's key from the org's endpoint registry — scoped, never pasted;
- `mcpConfig` gets the agent's existing MCP grants written in the harness's native config format at render time (zero in-sandbox reconnection);
- `mcpServe` registers the harness as a stdio MCP server on the agent's own Hermes config once the workbench image carries its binary — agents drive it with tools, not stdout;
- `install` hints feed the workbench image pipeline; `probe` (a cheap version command) is surfaced by the workbench **doctor** tool so agents can self-verify your harness runs;
- need a config format the built-ins don't cover? Set `mcpConfig.format: 'custom'` and export `renderMcpConfig(ctx)` — you get the agent's granted servers as per-agent gateway endpoints plus the API-key env var, and you return the JSON your harness reads, in your own env-substitution syntax (app-shipped harnesses only — admin JSON definitions can't carry code);
- jobs, branches, PRs, plan gates, per-job workspaces, and shared session history all behave identically — the harness is just the tool inside the flow.
