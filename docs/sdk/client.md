# Client — session and data

Everything here is imported from `@talaria/sdk`.

## Session

```ts
import { useMe, useHasPerm, useIsAdmin } from '@talaria/sdk'

const session = useMe()                        // a query; session.data is SessionUser | null
const mayUpload = useHasPerm('files.upload')   // a { current } box — read mayUpload.current
const admin = $derived(useIsAdmin(session.data))  // a plain predicate over a user, not a hook
```

`useHasPerm` returns `{ current }` rather than a bare boolean because a primitive computed once
at init would freeze before the session resolves; read `.current` in the template or inside
`$derived`. It defaults to `false` until then, so affordances appear rather than
flash-then-vanish.

## Platform APIs

```ts
import { api } from '@talaria/sdk'

// Runs as the signed-in user: every permission and ACL applies exactly as in the core UI.
const { agents } = await api<{ agents: Array<{ id: string; model: string }> }>('/api/agents')
```

`api()` throws on non-2xx with the server's `error` string. The full route catalog with auth
classes and body shapes: [`api/`](../api/README.md).

## Your app server

```ts
import { appApi, useAppQuery, useAppInvalidate } from '@talaria/sdk'

// Pre-bound to /api/apps/<slug>/… — get / post / put / patch / del
const mine = appApi('my-app')
await mine.post('things', { name: 'Ada' })

// svelte-query read; path may be a getter so reactive state re-keys the query
const query = useAppQuery<{ things: Doc[] }>('my-app', () => `things?q=${q}`)

// After a write — invalidate this app's queries
const invalidate = useAppInvalidate('my-app')
invalidate()            // everything
invalidate('things')    // one path
```

Call `useAppQuery` and `useAppInvalidate` during component init — they read the query client
from context; the function `useAppInvalidate` returns is safe anywhere.

## svelte-query

`createQuery`, `createMutation`, `useQueryClient`, `keepPreviousData` are re-exported from
`@tanstack/svelte-query` v6. Two rules that bite:

- Options are a **function**: `createQuery(() => ({ … }))`.
- Reactive sources must not be destructured — read `query.data`, never
  `const { data } = query`.

For anything beyond app-server reads, write it against platform APIs with `api()` and your own
query keys.
