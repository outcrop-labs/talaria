# @talaria/sdk — the app developer surface

Everything a [Talaria app](../APPS.md) imports. Two entry points:

| entry point | for | used in |
| :--- | :--- | :--- |
| `@talaria/sdk` | client: UI kit, session + data hooks, fetch helpers, motion | `app.ts`, `*.svelte` |
| `@talaria/sdk/server` | server: app server, document store, MCP tools, harnesses | `server.ts`, `mcp.ts`, `harness.ts`, `harnesses/*.ts` |

The stack is **Svelte 5 (runes) + sv-router + @tanstack/svelte-query v6 + Tailwind 4**. Both
entry points resolve from the host at build time — an app **installs nothing**; `svelte`,
svelte-query, zod, and `@lucide/svelte` (icons) all resolve from the host, exactly one copy in
the deployment. Import `svelte` and icons freely; import everything else through the SDK.

## The docset

| file | what it covers |
| :--- | :--- |
| [getting-started.md](./getting-started.md) | Anatomy, surfaces, your first app in ten minutes |
| [ui-kit.md](./ui-kit.md) | Every component group and when to use what |
| [client.md](./client.md) | Session hooks, data fetching, talking to your app server |
| [server.md](./server.md) | The fetch handler, the context, the document store |
| [mcp.md](./mcp.md) | Tools your app gives the org's agents |
| [harnesses.md](./harnesses.md) | Activity harnesses: model calls Talaria runs for you |
| [workbench-harnesses.md](./workbench-harnesses.md) | Workbench harnesses: coding tools agents drive |
| [reference.md](./reference.md) | Every export, one row each — enforced against the source |

The working reference app is [`apps/contacts`](../../apps/) in the repo: three surfaces, an app
server, the store, and agent tools.

## Security model

Apps compile into the deployment and run **fully trusted**, like platform code — admins install
only what they trust (the marketplace labels Outcrop-maintained apps **official**). The safety
boundary is the *session*: client calls run as the signed-in user; server handlers receive an
authenticated user; agent tools receive a gateway-authenticated agent identity. All existing
permissions, ACLs, and audit logging apply unchanged — an app can never do more than the person
using it.
