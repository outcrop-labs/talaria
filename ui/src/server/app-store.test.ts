// The app document store with an id that is not one (app-store.ts). Ids are
// the UUIDs `insert` mints; the docs table's id column is uuid, so Postgres
// refused any other string with a cast error and an app's `items/123` became
// a 500 "app crashed". The store now answers "no such doc" without asking.
import { describe, expect, it, vi } from 'vitest'

const appSql = vi.fn(async () => {
  throw new Error('the database was asked')
})
vi.mock('./app-db', () => ({ appSql }))

const { isDocId, storeFor } = await import('./app-store')

describe('the app store with a non-UUID id', () => {
  const store = storeFor('crm')
  const notIds = ['123', 'items', '', 'not-a-uuid', '0b3c7a2e-1f4d-4c8e-9a6b', '0b3c7a2e-1f4d-4c8e-9a6b-3d5e7f9a1c2bx', "1' or '1'='1"]

  it('get answers null, update null, remove false — and Postgres is never asked', async () => {
    for (const id of notIds) {
      expect(await store.get('items', id)).toBeNull()
      expect(await store.update('items', id, { a: 1 })).toBeNull()
      expect(await store.remove('items', id)).toBe(false)
    }
    expect(appSql).not.toHaveBeenCalled()
  })

  it('a UUID still reaches the database', async () => {
    const id = '0b3c7a2e-1f4d-4c8e-9a6b-3d5e7f9a1c2b'
    expect(isDocId(id)).toBe(true)
    expect(isDocId(id.toUpperCase())).toBe(true)
    await expect(store.get('items', id)).rejects.toThrow('the database was asked')
    expect(appSql).toHaveBeenCalledTimes(1)
  })

  it('isDocId is a guard against anything but a UUID string', () => {
    for (const v of [...notIds, null, undefined, 42, {}]) expect(isDocId(v)).toBe(false)
  })
})
