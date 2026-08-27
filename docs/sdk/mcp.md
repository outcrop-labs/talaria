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

## Governance — you get it, you're subject to it

An app's tools register as an MCP server entry and are governed in **Manage → MCP** exactly
like any server: an admin assigns which agents may call them, narrows the tool subset, and
gates people. Your handler receives a gateway-authenticated agent identity — never a user
session — and the same audit trail covers it. There is no "agent can call everything" mode
to opt out of.

The working example: [`apps/contacts/mcp.ts`](../../apps/).
