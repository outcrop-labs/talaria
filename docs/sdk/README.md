# @talaria/sdk — the app developer surface

Everything a [Talaria app](../APPS.md) imports. Two entry points:

| entry point | for | used in |
| :--- | :--- | :--- |
| `@talaria/sdk` | client: UI kit, session + data hooks, fetch helpers, motion | `app.ts`, `*.svelte` |
| `@talaria/sdk/server` | server: app server, document store, MCP tools, harnesses | `server.ts`, `mcp.ts`, and `harnesses/*.ts` |

The stack is **Svelte 5 (runes) + @tanstack/svelte-query v6 + Tailwind 4**, rendered inside the
host's own router. Both entry points resolve from the host at build time — an app **installs
nothing**; `svelte`, svelte-query, zod, and `@lucide/svelte` (icons) all resolve from the host,
exactly one copy in the deployment. Import `svelte` and icons freely; import everything else
through the SDK — routing included: an app's routes are the host's, reached through
`useAppRoute`, `appHref` and `appNavigate` ([client.md](./client.md#routes-inside-your-app)),
never by importing a router of its own.

## The docset

| file | what it covers |
| :--- | :--- |
| [getting-started.md](./getting-started.md) | Anatomy, surfaces, your first app in ten minutes |
| [ui-kit.md](./ui-kit.md) | Every component group and when to use what |
| [client.md](./client.md) | Session hooks, data fetching, talking to your app server |
| [server.md](./server.md) | The fetch handler, the context, grants, the document store, the platform API, secrets |
| [mcp.md](./mcp.md) | Tools your app gives the org's agents |
| [harnesses.md](./harnesses.md) | Activity harnesses: model calls Talaria runs for you |
| [reference.md](./reference.md) | Every export, one row each — enforced against the source |
| [outside-the-host.md](./outside-the-host.md) | Building an app outside the host: the SDK kit, `talaria-app`, the sandbox loop |

Scaffold a new app with `bun talaria app new <slug>` in a Talaria checkout, or with the SDK kit's
`talaria-app scaffold` in a repo of its own ([outside-the-host.md](./outside-the-host.md)).

## Security model

Apps run **fully trusted**, like platform code — admins install only what they
trust (the marketplace labels Outcrop-maintained apps **official**). The safety
boundary is the *session*: client calls run as the signed-in user; server handlers receive an
authenticated user and the app surfaces they hold (`ctx.access`), and `ctx.api` calls the platform
as that user; agent tools receive the calling agent, flagged proven only when it called on its own
key, and the person it acts for. All existing
permissions, ACLs, and audit logging apply unchanged — an app can never do more than the person
using it.
