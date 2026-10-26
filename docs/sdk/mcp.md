# MCP — tools for agents

`apps/<slug>/mcp.ts` default-exports `defineAppMcp({...})`: tools the org's **agents** can
call. Calls dispatch in-process with your app's store — no separate server process.

```ts
// apps/<slug>/mcp.ts
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
        const query = String(args.query ?? '').toLowerCase()
        const hits = await ctx.store.list('contacts', { limit: 500 })
        return hits
          .filter((c) => JSON.stringify(c.data).toLowerCase().includes(query))
          .map((c) => ({ id: c.id, ...c.data }))
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
| `outputSchema` | Optional JSON Schema (`type: "object"`) for a structured result — see [Results](#results) |
| `handler(args, ctx)` | The call: the arguments, and [who is calling, for whom, until when](#the-handlers-context) |

**Each tool is checked when the app loads, and a tool that fails is left out — never the app.**
A name is required, unique within the app, at most 200 characters, and only letters, digits, `_`
and `-` (an agent's model sees it as a function name, and providers refuse a whole request over
one with a space or a dot). A description is required. `inputSchema`, when given, describes an
object and is at most 32 KiB of JSON; so does `outputSchema`, which must say `type: "object"`.
`handler` is a function. A tool that fails any of these is
not listed, not callable and not in the registry's catalog; the reason is logged and shown on the
app's server card in **Manage → MCP**, and the app's other tools keep working.

`inputSchema` is checked before your handler runs: a call whose arguments do not satisfy it is
answered as a tool error naming the field, and your handler never sees it. Schemas the checker
cannot compile (a `$ref`, an unknown draft) are passed through unchecked.

## The handler's context

| `ctx.` | what it is |
| :--- | :--- |
| `app` | Your app's slug |
| `caller` | `{ name, id, proven }` — the calling agent's fleet name, its id, and whether the gateway proved it |
| `agent` | `caller.name`, kept for tools written before `caller` |
| `actingUserId` | The person the agent is acting for, when the gateway knows it — else `null` |
| `store` | Your document store, the same one `server.ts` gets |
| `signal` | Aborts at the server's deadline, or when the agent disconnects |
| `harness` | Runs a harness for this call. It may run on an agent only if that agent is the caller, proven ([harnesses](./harnesses.md#running-one--ctxharness)) |

**`caller.proven` is true only when the call came in on that agent's own gateway key.** An agent
still on the org-wide `TALARIA_AGENT_KEY` (the legacy shared key, on until an admin closes it) is
identified by the name it put in its `X-Agent-Name` header: `caller.name` is that claim,
`caller.id` is `null`, and `proven` is `false`. Decide nothing that matters on an unproven caller
— whose data a call may read or change, what it may spend. The gateway itself spends no secret
for one.

**`actingUserId` is the human a personal assistant belongs to.** Scope that person's data by it,
the way a `server.ts` route scopes by `ctx.user`. It is `null` for an org agent, which acts for
the org rather than one person, and for an unproven caller. It is the same person the call
record attributes the call to.

**A handler runs under a deadline: the server's timeout, 120 s unless an admin set another in
Manage → MCP.** When it passes, `ctx.signal` aborts and the agent is answered at once with a tool
error that says the call was cut off; when the agent disconnects first, the signal aborts too and
the call is recorded as cancelled. A call whose agent left before it began never runs the handler.
Pass the signal to anything long, such as `fetch(url, { signal: ctx.signal })`, so the work stops
with the call. `ctx.harness` follows it already. JavaScript cannot stop a handler that ignores
it: that one runs to its end, and nothing it returns reaches anyone. A tool that can outlast the default needs an admin to raise the server's timeout.

## Results

| the handler returns | the agent receives |
| :--- | :--- |
| a string | one text block |
| any other value | its JSON, pretty-printed, as one text block |
| `toolResult({ content?, structuredContent?, isError? })` | that result, unchanged |
| a throw | a tool error (`isError`) whose text is `error: <message>` |

Throw `Error('reason')` to fail — the message is what the agent sees. Keep results small: this
lands in a model's context.

**`toolResult(...)` is the whole MCP result, for a tool that needs more than one text block.**
`content` is a list of blocks — `text`, `image` and `audio` (base64 `data` plus `mimeType`),
`resource_link` (`uri`, `name`) and `resource` (an embedded `uri` with `text` or `blob`).
`structuredContent` is typed data a client reads without parsing text; given without `content`,
its JSON is added as a text block for clients that read only text. `isError: true` reports a
failure without a throw.

```ts
import { defineAppMcp, toolResult } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'contacts_count',
      description: 'Count contacts. Returns { count }.',
      outputSchema: {
        type: 'object',
        properties: { count: { type: 'integer' } },
        required: ['count'],
      },
      async handler(_args, ctx) {
        const count = await ctx.store.count('contacts')
        if (count === 0) {
          return toolResult({ content: [{ type: 'text', text: 'No contacts yet.' }], isError: true })
        }
        return { count }
      },
    },
  ],
})
```

**A tool that declares an `outputSchema` answers structured.** Its plain object return is the
`structuredContent` (with its JSON as the text block), and whatever it returns is checked against
the schema before the agent sees it. A result the schema refuses, a tool with an `outputSchema`
that returns no object, or a `toolResult` whose content is not a block MCP defines, is not a tool
error but the server's: the agent gets JSON-RPC error -32603 naming the tool and what broke, and
the call record keeps that sentence. A schema refusal is named by zod's issue codes and their
counts (`1 × invalid_type, 1 × unrecognized_keys`), never a key, path or value of the result, so
the record never holds the tool's data. An `isError` result is not checked against the schema.

## Secrets in arguments

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
never tells the agent. The handle text can sit anywhere in a string, inside a header value or a
URL as well as on its own, so check before you spend an argument as a credential:
`isUnresolvedHandle(value)` from `@talaria/sdk/server` is true when the value — a string, or any
string inside an array or object — still carries a `«secret:` handle. Fail with a sentence that
names the secret, rather than sending the handle upstream and returning the auth error you get
back:

```ts
import { defineAppMcp, isUnresolvedHandle } from '@talaria/sdk/server'

export default defineAppMcp({
  tools: [
    {
      name: 'crm_sync',
      description: 'Pull contacts from the CRM. Pass the CRM key as apiKey.',
      inputSchema: {
        type: 'object',
        properties: { apiKey: { type: 'string' } },
        required: ['apiKey'],
      },
      async handler(args, ctx) {
        if (isUnresolvedHandle(args.apiKey)) {
          throw new Error(
            'the CRM key did not resolve: its Spendable at list must name talaria-app://crm',
          )
        }
        const res = await fetch('https://api.crm.example/contacts', {
          headers: { authorization: `Bearer ${String(args.apiKey)}` },
          signal: ctx.signal,
        })
        return { status: res.status }
      },
    },
  ],
})
```

## Governance — you get it, you're subject to it

An app's tools register as an MCP server entry and are governed in **Manage → MCP** exactly
like any server: an admin assigns which agents may call them, narrows the tool subset, and
gates people. Your handler receives the calling agent — proven or merely identified, as
`ctx.caller.proven` says — and never a user session, and the same audit trail covers it: every
`tools/call` — the refused ones too — lands as a row in `mcp_tool_calls` (agent, acting person,
server, tool, outcome, latency; never the arguments or the result), the same table Observability
reads for every other server. There is no "agent can call everything" mode to opt out of.

The gateway answers what it will not dispatch the way it does for every other server: a body that
is not JSON gets JSON-RPC -32700 with a bounded hexdump of where it broke, a batch gets -32600 per
request (each `tools/call` in it recorded as refused), and a request that is not an object, or
names no method, gets -32600. Your handler sees none of them.

**When your tools change, the agents that carry your server are restarted to see them.** An
agent lists a server's tools once, when it starts. When an update or a rebuild changes the
catalog — a tool added, renamed or removed, a description or a schema edited — Talaria rolls
every agent granted the server, so each lists the new catalog; a rebuild that leaves the catalog
as it was restarts nobody.

## Testing your tools

`@talaria/sdk/testing` runs a tool the way the gateway does, with no host: the load checks, the
`inputSchema` check, the handler under its deadline and signal, and the shaping of its result.
`callAppTool(mcp, name, args, ctx?)` answers what the agent would receive — `content`,
`structuredContent`, `isError`, the joined `text`, whether the call was `cut`, and `cutBy`: the
`"deadline"` or the `"caller"` leaving (your `signal`) — and rejects with the gateway's own
sentence where the gateway would answer an error (a tool the checks leave out, a result that
breaks its `outputSchema`). `memoryStore()` is a document store with the installed store's
interface and answers, in memory.

```ts
// apps/<slug>/mcp.test.ts
import { expect, test } from 'vitest'
import { callAppTool, memoryStore } from '@talaria/sdk/testing'
import mcp from './mcp'

test('search finds a stored contact', async () => {
  const store = memoryStore()
  await store.insert('contacts', { name: 'Ada Lovelace' })
  const r = await callAppTool(mcp, 'contacts_search', { query: 'ada' }, { store })
  expect(r.isError).toBe(false)
  expect(r.text).toContain('Ada Lovelace')
})

test('an empty store finds nothing', async () => {
  const r = await callAppTool(mcp, 'contacts_search', { query: 'ada' })
  expect(r.text).toBe('[]')
})
```

The context defaults to a proven agent (`test-agent`), no acting person, a fresh `memoryStore()`
and the 120 s deadline. `caller: { proven: false }` is an agent on the org-wide key, with the
null `id` the gateway gives it. Set any of `caller`, `actingUserId`, `store`, `timeoutSecs`,
`signal`, or `tools` (the agent's granted subset) to test the case it decides. Run an app's tests
from `ui/`: `TALARIA_APP_TESTS=<slug> bunx vitest run` runs `apps/<slug>/**/*.test.ts` and nothing
else. `@talaria/sdk/testing` is for tests only: an app build does not resolve it.

The starter: `bun talaria app new <slug>` writes an `mcp.ts`.
