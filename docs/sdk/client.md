# Client — session and data

Everything here is imported from `@talaria/sdk`. Every `svelte` and `ts` example on this page
is compiled against the SDK by the ui test suite (`ui/src/sdk/client-docs.test.ts`), so a
copied example compiles as written.

## Which app am I

A surface never spells its own slug. The slug is whatever THIS instance installed the app
under — install derives it from the repo name, so an app scaffolded as `contacts` and published
as `contacts-app` runs as `contacts-app`. The host tells the surface instead:

```svelte
<script lang="ts">
  import { useApp } from '@talaria/sdk'

  const app = useApp() // { slug, surface, manifest } — read live, don't destructure
</script>

<p>{app.manifest.name} ({app.slug}), {app.surface} surface</p>
```

Every hook below takes the slug as an optional first argument and defaults to the surrounding
surface's. Outside a surface (a plain `.ts` helper module called from no component), pass it.

## Session

```svelte
<script lang="ts">
  import { useHasPerm, useIsAdmin, useMe } from '@talaria/sdk'

  const session = useMe() // a query; session.data is SessionUser | null
  const mayUpload = useHasPerm('files.upload') // a { current } box — read mayUpload.current
  const admin = $derived(useIsAdmin(session.data)) // a plain predicate over a user, not a hook
</script>

{#if admin}<p>Admin</p>{/if}
{#if mayUpload.current}<p>You can upload files.</p>{/if}
```

`useHasPerm` returns `{ current }` rather than a bare boolean because a primitive computed once
at init would freeze before the session resolves; read `.current` in the template or inside
`$derived`. It defaults to `false` until then, so affordances appear rather than
flash-then-vanish.

## Platform APIs

```ts
import { api, HttpError } from '@talaria/sdk'

// Runs as the signed-in user: every permission and ACL applies exactly as in the core UI.
const { agents } = await api<{ agents: Array<{ id: string; label: string; role: string; tiers: string[] }> }>(
  '/api/agents',
)

// A 404 that means "none" is data, not a failure — tell it apart by status.
export async function findBoard(id: string): Promise<{ board: { id: string; name: string } } | null> {
  try {
    return await api<{ board: { id: string; name: string } }>(`/api/boards/${id}`)
  } catch (e) {
    if (e instanceof HttpError && e.status === 404) return null
    throw e
  }
}

export const agentCount = agents.length
```

`api()` is the host's own HTTP door. A non-2xx rejects with an `HttpError` carrying `status`
and the server's `error` sentence as its message; a 2xx that is not JSON rejects with a sentence
that says so. Reads stop after 30 seconds; give a write `timeout: ms` or a `signal` when it
should stop too. `errorMessage(e)` turns any thrown value into one line for the UI. The full
route catalog with auth classes and body shapes: [`api/`](../api/README.md).

## Your app server

```svelte
<script lang="ts">
  import { Button, Input, QueryState, SkeletonRows, alert, errorMessage } from '@talaria/sdk'
  import { useAppApi, useAppInvalidate, useAppQuery } from '@talaria/sdk'

  type Thing = { id: string; data: { name: string } }

  let q = $state('')
  // A read of GET /api/apps/<slug>/things?q=… — a getter re-keys it as q changes.
  const things = useAppQuery<{ things: Thing[] }>(() => `things?q=${encodeURIComponent(q)}`)
  const mine = useAppApi() // get / post / put / patch / del, bound to /api/apps/<slug>/
  const invalidate = useAppInvalidate()

  async function add(name: string) {
    try {
      await mine.post('things', { name })
      await invalidate('things') // every read under things: things?q=…, things/123, …
    } catch (e) {
      await alert({ title: 'Could not add it', message: errorMessage(e) })
    }
  }
</script>

<Input bind:value={q} placeholder="Search" />
<Button onclick={() => void add('Ada')}>Add Ada</Button>
<QueryState query={things} isEmpty={(d) => d.things.length === 0}>
  {#snippet skeleton()}<SkeletonRows rows={4} />{/snippet}
  {#snippet empty()}<p>No things yet.</p>{/snippet}
  {#snippet children(data)}
    {#each data.things as t (t.id)}<div>{t.data.name}</div>{/each}
  {/snippet}
</QueryState>
```

- `useAppQuery(path, options?)` reads an app-server GET. `path` may be a getter so reactive
  state re-keys the query; `options` passes `enabled`, `select`, `staleTime`, `gcTime`,
  `refetchInterval`, `refetchOnWindowFocus`, `placeholderData` and `retry` through to
  svelte-query, and may be a getter too, for a reactive `enabled`.
- Query keys are built from the path's segments, then its search params:
  `things/123?tab=notes` reads under `['app', slug, 'things', '123', { tab: 'notes' }]`
  (`appQueryKey` builds it, for your own `createQuery`). `invalidate('things')` therefore
  reaches every read below `things`; `invalidate()` reaches all of the app's reads.
- Call the three hooks during component init — they read the surface and the query client from
  context. What they return is safe anywhere. `appApi(slug)` is the same verbs for code
  outside a component.
- `useAppApi().url('events')` is the absolute URL of a route, for the streaming helpers below.

## Rendering a read: loading, broken, or empty

A read that failed must never render as "there are none of these". The host's rule is the
SDK's: hand the whole query to `QueryState` with your own skeleton, and the error branch comes
for free (an `HttpError`'s sentence and a Retry button). For a list you lay out yourself,
`listQuery` returns the rows together with the failure, already shaped:

```svelte
<script lang="ts">
  import { QueryError, SkeletonRows, listQuery, useAppQuery } from '@talaria/sdk'

  type Item = { id: string; data: { title: string } }

  const items = useAppQuery<{ items: Item[] }, Item[]>('items', { select: (d) => d.items })
  const list = listQuery(items, { title: 'Could not load the items' })
</script>

{#if list.pending}
  <SkeletonRows rows={5} />
{:else if list.failed && list.notice}
  <QueryError {...list.notice} />
{:else}
  {#if list.notice}<QueryError {...list.notice} />{/if}
  {#each list.rows as item (item.id)}<div>{item.data.title}</div>{/each}
{/if}
```

`listQuery` keeps stale rows on screen with an inline "may be out of date" note when a
background refetch fails; `failed` is true only when there is nothing to show.

## Live events

```svelte
<script lang="ts">
  import { useAppInvalidate, useUserEvents } from '@talaria/sdk'

  const invalidate = useAppInvalidate()
  // run, notification, brief, channel, conversation and typing transitions — ids only
  useUserEvents((event) => {
    if (event.type === 'run') void invalidate('runs')
  })
</script>
```

The tab holds one shared connection to `/api/me/events`; `useUserEvents` subscribes for the
component's lifetime. Never open a second `EventSource` on that stream — each is a dedicated
subscriber on the server. Events carry ids only: refetch, don't patch.

## Streaming

`getStream(url)` and `postStream(url, body)` return the open `Response` for a streamed reply,
with the same error contract as `api()`; `openStream(url, onMessage)` subscribes to a
server-sent-events route and returns the unsubscribe. Point them at your own server with
`useAppApi().url(path)`.

## svelte-query

`createQuery`, `createMutation`, `useQueryClient`, `keepPreviousData` are re-exported from
`@tanstack/svelte-query` v6. Two rules that bite:

- Options are a **function**: `createQuery(() => ({ … }))`.
- Reactive sources must not be destructured — read `query.data`, never
  `const { data } = query`.

For anything beyond app-server reads, write it against platform APIs with `api()` and your own
query keys.
