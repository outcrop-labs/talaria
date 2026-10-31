// What an app document's id is. A leaf: the Postgres store (app-store.ts) and
// the in-memory one a unit test uses (app-store-memory.ts) both read it.

/** The ids `insert` mints (`randomUUID`), the only ids a doc can have: the
 *  docs table's `id` column is `uuid`. Anything else names no document, and
 *  `get`/`update`/`remove` answer so without asking Postgres — which would
 *  refuse the cast (22P02) and turn an app's `items/123` into a 500 "app
 *  crashed" instead of its own 404. */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

export const isDocId = (id: unknown): id is string => typeof id === 'string' && UUID.test(id)
