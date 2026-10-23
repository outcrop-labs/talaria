// An app's document store, in memory — `memoryStore()` from
// @talaria/sdk/testing. The same interface and the same answers as the
// Postgres store an installed app gets (app-store.ts `storeFor`), so a unit
// test of a tool reads what production would: documents come back as JSON
// copies (what jsonb stores), ids are UUIDs and anything else names no
// document, `update` merges one level deep (jsonb `||`), and `list` pages and
// orders by creation the same way. Nothing outlives the store object.
import { randomUUID } from 'node:crypto'
import { isDocId } from './app-doc-id'
import type { AppDoc, AppStore } from './app-store'

interface Row {
  id: string
  data: Record<string, unknown>
  createdAt: string
  updatedAt: string
  /** Insertion order: two docs created in the same millisecond still list in
   *  the order they were inserted. */
  seq: number
}

/** A value as jsonb would hand it back: undefined dropped, Dates as strings,
 *  every object a fresh copy. */
const asJson = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T

export function memoryStore(): AppStore {
  const collections = new Map<string, Map<string, Row>>()
  let seq = 0
  const docs = (collection: string): Map<string, Row> => {
    let c = collections.get(collection)
    if (!c) collections.set(collection, (c = new Map()))
    return c
  }
  const out = <T>(r: Row): AppDoc<T> => ({ id: r.id, data: asJson(r.data) as T, createdAt: r.createdAt, updatedAt: r.updatedAt })

  return {
    async list<T>(collection: string, opts?: { limit?: number; offset?: number; newestFirst?: boolean }) {
      const limit = Math.min(Math.max(opts?.limit ?? 200, 1), 1000)
      const offset = Math.max(opts?.offset ?? 0, 0)
      const rows = [...docs(collection).values()].sort((a, b) => a.seq - b.seq)
      if (opts?.newestFirst ?? true) rows.reverse()
      return rows.slice(offset, offset + limit).map((r) => out<T>(r))
    },
    async get<T>(collection: string, id: string) {
      if (!isDocId(id)) return null
      const r = docs(collection).get(id.toLowerCase())
      return r ? out<T>(r) : null
    },
    async insert<T extends Record<string, unknown>>(collection: string, data: T) {
      const now = new Date().toISOString()
      const r: Row = { id: randomUUID(), data: asJson(data), createdAt: now, updatedAt: now, seq: seq++ }
      docs(collection).set(r.id, r)
      return out<T>(r)
    },
    async update<T>(collection: string, id: string, patch: Record<string, unknown>) {
      if (!isDocId(id)) return null
      const r = docs(collection).get(id.toLowerCase())
      if (!r) return null
      r.data = { ...r.data, ...asJson(patch) }
      r.updatedAt = new Date().toISOString()
      return out<T>(r)
    },
    async remove(collection: string, id: string) {
      if (!isDocId(id)) return false
      return docs(collection).delete(id.toLowerCase())
    },
    async count(collection: string) {
      return docs(collection).size
    },
    async wipe() {
      collections.clear()
    },
  }
}
