// Shared types + query for the Storage admin panel
// (StoragePanel/TargetFields.svelte).
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'
import { formatBytes } from '@/lib/format'

export interface TargetConfig {
  endpoint: string
  region: string
  bucket: string
  accessKeyId: string
  pathStyle: boolean
  prefix: string
  hasSecret: boolean
}
export interface JobStatus {
  running: boolean
  moved: number
  failed: number
  total: number
  error?: string
}
export interface StorageAdmin {
  config: TargetConfig & { mode: 'local' | 'internal' | 's3'; replica: TargetConfig & { enabled: boolean } }
  stats: { local: number; s3: number; internal: number; localBytes: number }
  migrate: JobStatus | null
  sync: JobStatus | null
  internal: { endpoint: string; bucket: string }
}

export const useStorageAdmin = () =>
  createQuery(() => ({
    queryKey: ['storage-admin'],
    queryFn: (): Promise<StorageAdmin> => getJson<StorageAdmin>('/api/admin/storage'),
    refetchInterval: (q) => (q.state.data?.migrate?.running || q.state.data?.sync?.running ? 3_000 : false),
  }))

/** A byte count in the storage panel's convention: never below a KB (a 1-byte
 *  object is not "0 B"), rounded up, and a GB tier because a bucket is not a
 *  single upload. The spelling is shared — see `formatBytes` in `@/lib/format`;
 *  StoragePanel imports it by this name, so the name stays. */
export const fmtBytes = (n: number) => formatBytes(n, { ceilKb: true })
