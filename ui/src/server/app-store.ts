// Per-app document store — the "database" every Talaria app gets for free.
// One shared table (app_data), namespaced by app slug and collection name;
// docs are JSON with server-managed timestamps. Enough to build real tools
// without migrations; apps with heavier needs can still call platform APIs.
import { db } from './db/pg'

export interface AppDoc<T = Record<string, unknown>> {
  id: string
  data: T
  createdAt: string
  updatedAt: string
}

export interface AppStore {
  list<T = Record<string, unknown>>(collection: string, opts?: { limit?: number; offset?: number; newestFirst?: boolean }): Promise<AppDoc<T>[]>
  get<T = Record<string, unknown>>(collection: string, id: string): Promise<AppDoc<T> | null>
  insert<T extends Record<string, unknown>>(collection: string, data: T): Promise<AppDoc<T>>
  /** Shallow-merge a patch into the doc. Returns null when the doc is gone. */
  update<T = Record<string, unknown>>(collection: string, id: string, patch: Record<string, unknown>): Promise<AppDoc<T> | null>
  remove(collection: string, id: string): Promise<boolean>
  count(collection: string): Promise<number>
  /** Drop EVERYTHING this app stored (uninstall / danger zone). */
  wipe(): Promise<void>
}

const ROW = `id, data, created_at as "createdAt", updated_at as "updatedAt"`

export function storeFor(app: string): AppStore {
  return {
    async list(collection, opts) {
      const sql = await db()
      const limit = Math.min(Math.max(opts?.limit ?? 200, 1), 1000)
      const offset = Math.max(opts?.offset ?? 0, 0)
      const dir = (opts?.newestFirst ?? true) ? sql.unsafe('desc') : sql.unsafe('asc')
      return (await sql`
        select ${sql.unsafe(ROW)} from app_data
        where app = ${app} and collection = ${collection}
        order by created_at ${dir} limit ${limit} offset ${offset}
      `) as never
    },
    async get(collection, id) {
      const sql = await db()
      const rows = (await sql`
        select ${sql.unsafe(ROW)} from app_data where app = ${app} and collection = ${collection} and id = ${id}
      `) as unknown as AppDoc[]
      return (rows[0] as never) ?? null
    },
    async insert(collection, data) {
      const sql = await db()
      const rows = (await sql`
        insert into app_data (app, collection, data) values (${app}, ${collection}, ${sql.json(data as never)})
        returning ${sql.unsafe(ROW)}
      `) as unknown as AppDoc[]
      return rows[0] as never
    },
    async update(collection, id, patch) {
      const sql = await db()
      const rows = (await sql`
        update app_data set data = data || ${sql.json(patch as never)}, updated_at = now()
        where app = ${app} and collection = ${collection} and id = ${id}
        returning ${sql.unsafe(ROW)}
      `) as unknown as AppDoc[]
      return (rows[0] as never) ?? null
    },
    async remove(collection, id) {
      const sql = await db()
      const rows = await sql`
        delete from app_data where app = ${app} and collection = ${collection} and id = ${id} returning id
      `
      return rows.length > 0
    },
    async count(collection) {
      const sql = await db()
      const rows = (await sql`
        select count(*)::int as n from app_data where app = ${app} and collection = ${collection}
      `) as unknown as Array<{ n: number }>
      return rows[0]?.n ?? 0
    },
    async wipe() {
      const sql = await db()
      await sql`delete from app_data where app = ${app}`
    },
  }
}
