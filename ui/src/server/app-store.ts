// Per-app document store. Each app talks to its OWN Postgres (a docker
// container spawned for that install — see app-db.ts), never to Talaria's
// catalog. The API is unchanged: collections of JSON documents, no
// migrations, invisible to other apps.
import { randomUUID } from 'node:crypto'
import { appSql } from './app-db'

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

function iso(v: unknown): string {
  if (v instanceof Date) return v.toISOString()
  if (typeof v === 'string') return v
  return String(v ?? '')
}

function asDoc<T>(row: { id: string; data: T; createdAt: unknown; updatedAt: unknown }): AppDoc<T> {
  return { id: row.id, data: row.data, createdAt: iso(row.createdAt), updatedAt: iso(row.updatedAt) }
}

export function storeFor(app: string): AppStore {
  return {
    async list(collection, opts) {
      const sql = await appSql(app)
      const limit = Math.min(Math.max(opts?.limit ?? 200, 1), 1000)
      const offset = Math.max(opts?.offset ?? 0, 0)
      const dir = (opts?.newestFirst ?? true) ? sql.unsafe('desc') : sql.unsafe('asc')
      const rows = (await sql`
        select ${sql.unsafe(ROW)} from docs
        where collection = ${collection}
        order by created_at ${dir} limit ${limit} offset ${offset}
      `) as unknown as Array<{ id: string; data: Record<string, unknown>; createdAt: unknown; updatedAt: unknown }>
      return rows.map((r) => asDoc(r)) as never
    },
    async get(collection, id) {
      const sql = await appSql(app)
      const rows = (await sql`
        select ${sql.unsafe(ROW)} from docs where collection = ${collection} and id = ${id}
      `) as unknown as Array<{ id: string; data: Record<string, unknown>; createdAt: unknown; updatedAt: unknown }>
      return rows[0] ? (asDoc(rows[0]) as never) : null
    },
    async insert(collection, data) {
      const sql = await appSql(app)
      const id = randomUUID()
      const rows = (await sql`
        insert into docs (collection, id, data) values (${collection}, ${id}, ${sql.json(data as never)})
        returning ${sql.unsafe(ROW)}
      `) as unknown as Array<{ id: string; data: Record<string, unknown>; createdAt: unknown; updatedAt: unknown }>
      return asDoc(rows[0]!) as never
    },
    async update(collection, id, patch) {
      const sql = await appSql(app)
      const rows = (await sql`
        update docs set data = data || ${sql.json(patch as never)}, updated_at = now()
        where collection = ${collection} and id = ${id}
        returning ${sql.unsafe(ROW)}
      `) as unknown as Array<{ id: string; data: Record<string, unknown>; createdAt: unknown; updatedAt: unknown }>
      return rows[0] ? (asDoc(rows[0]) as never) : null
    },
    async remove(collection, id) {
      const sql = await appSql(app)
      const rows = await sql`
        delete from docs where collection = ${collection} and id = ${id} returning id
      `
      return rows.length > 0
    },
    async count(collection) {
      const sql = await appSql(app)
      const rows = (await sql`
        select count(*)::int as n from docs where collection = ${collection}
      `) as unknown as Array<{ n: number }>
      return rows[0]?.n ?? 0
    },
    async wipe() {
      const sql = await appSql(app)
      await sql`truncate docs`
    },
  }
}
