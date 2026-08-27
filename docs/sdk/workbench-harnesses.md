# Workbench harnesses — coding tools agents drive

Two contracts wore the name "harness", and they are not specializations of each other:

| | [Activity harness](./harnesses.md) | Workbench harness |
| :--- | :--- | :--- |
| Who runs it | **Talaria** — one model call, executed by `runHarness` | **An agent** — a coding CLI in a sandbox |
| File | `harnesses/*.ts` — one per file | `harness.ts` — one per app |
| Carries | Code: `render`, `output`, `evals` | Data: shell templates, env, MCP config |
| Identity | `id` in the activity registry | `slug` in the harness registry (profiles, per-agent picks) |
| Declares | Prompt, output contract, model chain, failure policy | Invocation, auth, MCP pass-through, a guide |
| Typed | `defineHarness<I, O>` | `defineWorkbenchHarness` |

`defineHarness` still accepts the workbench shape (deprecated overload) so existing apps
keep building — new code says which contract it means by name.

## The definition

```ts
import { defineWorkbenchHarness } from '@talaria/sdk/server'

export default defineWorkbenchHarness({
  slug: 'myapp-runner',
  label: 'MyApp Runner',
  description: 'Runs MyApp fixtures against a repo checkout.',
  auth: 'gateway',                       // or { provider, envVar } for an org endpoint
  invoke: 'myapp run --model <model> <task>',
  jsonInvoke: 'myapp run --json --model <model> <task>',
  mcpServe: { command: 'myapp', args: ['mcp'] },
  mcpConfig: { format: 'claude-json', filename: '.mcp.json' },
  probe: 'myapp --version',
  guide: 'Sessions live in .myapp/. One task per invocation. JSON results are authoritative.',
})
```

| field | what it declares |
| :--- | :--- |
| `slug` / `label` / `description?` | Registry identity — `slug` is what profiles and per-agent picks reference |
| `auth` | `'gateway'` (Talaria's metered gateway) or `{ provider, envVar }` from the org's endpoint registry |
| `env?` | Extra container env, compose-interpolated, merged over auth env |
| `invoke` | Invocation template — `<model>` and `<task>` placeholders |
| `jsonInvoke?` | Structured-output form — **required for good drivers**; agents read structured results, never scrape logs |
| `mcpServe?` | How to run the harness itself as an MCP server (stdio) — the preferred integration |
| `mcpConfig?` | MCP pass-through the harness reads: `{ format: 'claude-json' \| 'opencode-json' \| 'custom', filename }` |
| `renderMcpConfig?` | Custom renderer for `format: 'custom'` (app-shipped only) — you own env-substitution |
| `modelPrefix?` | Prefix model ids need (e.g. `openai/`) |
| `probe` | A cheap command that proves it runs — the workbench doctor surfaces it |
| `guide` | What a driving agent should understand: sessions, resume, results |
| `install?` | RESERVED — image-build layer hints, validated now, consumed later |

The host merges your definition into the harness registry (builtin < app-shipped <
admin-custom, by slug): it becomes selectable per agent, its auth/env provision into the
sandbox at render time, and — where it can serve MCP — it registers on the agent's config as
stdio tools. **Declarative only: no host code runs from the definition.**

`renderMcpConfig` receives a `HarnessMcpRenderContext`: `{ agentModel, servers, apiKeyEnvVar }`
— the agent's granted MCP servers as per-agent gateway endpoints, and the env var holding the
fleet key for your own substitution syntax.

The deep contract, including agent-side behavior: [WORKBENCH.md](../WORKBENCH.md).
