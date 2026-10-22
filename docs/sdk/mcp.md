# MCP — tools for agents

`apps/<slug>/mcp.ts` default-exports `defineAppMcp({...})`: tools the org's **agents** can
call. Calls dispatch in-process with your app's store — no separate server process.

```ts
import { defineAppMcp } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'contacts_search',
      description: 'Search the CRM by name, company, email, or notes.',
      inputSchema: {
        type: 'object',
        properties: { query: { type: 'string', description: 'Text to match' } },
      },
      async handler(args, ctx) {
        const hits = await ctx.store.list('contacts', { limit: 500 })
        return hits.map((c) => ({ id: c.id, ...c.data }))
      },
    },
  ],
})
```

## Tool shape

| field | what it is |
| :--- | :--- |
| `name` | Globally unique tool id — prefix it (`contacts_search`, not `search`) |
| `description` | What the agent reads to decide to call it — say what it returns |
| `inputSchema` | JSON Schema (the MCP wire shape — not zod) |
| `handler(args, ctx)` | The call. `ctx.store` is your document store; `ctx.agent` is the calling agent's name |

Return JSON-serializable values; throw `Error('reason')` to fail — the message is what the
agent sees. Keep results small: this lands in a model's context.

**An argument may arrive carrying a resolved credential, and your handler must not persist it.**
An agent holds workspace secrets as handles (`«secret:crm.api_key»`) and passes one wherever the
value would go; the gateway resolves the handle on its way into your handler, so `args` can hold
the real key. Use it for the call it was sent for and let it go: never `ctx.store.insert(…, args)`
for provenance, never log the arguments, never echo them in a thrown message. The call record
Talaria keeps for you (`mcp_tool_calls`) stores the tool name and outcome and never the arguments,
for the same reason.

**Your tools are a destination named `talaria-app://<slug>`, and a locked secret has to list it.**
A secret's **Spendable at** list (Admin → Secrets) says where it may go. Empty means anywhere. A
non-empty list must name the destination — for your tools, `talaria-app://<slug>` exactly — and
every host the arguments mention: a CRM key locked to `api.crm.com` resolves for a `crm` app tool
only once the list reads `api.crm.com talaria-app://crm`. When a handle does not resolve — that
list, an agent without a grant on the secret, an expired secret, or an agent still calling with
the org-wide `TALARIA_AGENT_KEY` instead of its own key — **your handler receives the handle text
itself**, `«secret:crm.api_key»`, in place of the value. The gateway logs why for the operator and
never tells the agent. Check for the `«secret:` prefix before you spend an argument as a
credential, and fail with a sentence that names the secret, rather than sending the handle
upstream and returning the auth error you get back.

`inputSchema` is checked before your handler runs: a call whose arguments do not satisfy it is
answered as a tool error naming the field, and your handler never sees it. Schemas the checker
cannot compile (a `$ref`, an unknown draft) are passed through unchecked.

## Governance — you get it, you're subject to it

An app's tools register as an MCP server entry and are governed in **Manage → MCP** exactly
like any server: an admin assigns which agents may call them, narrows the tool subset, and
gates people. Your handler receives a gateway-authenticated agent identity — never a user
session — and the same audit trail covers it: every `tools/call` — the refused ones too — lands as
a row in `mcp_tool_calls` (agent, acting person, server, tool, outcome, latency; never the
arguments or the result), the same table Observability reads for every other server. There is no "agent can call everything" mode
to opt out of.

The starter: `bun talaria app new <slug>` writes an `mcp.ts`.
