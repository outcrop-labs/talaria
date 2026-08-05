// Shared types + query for the Storage admin panel
// (StoragePanel/TargetFields.svelte).
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

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

export const fmtBytes = (n: number) => (n >= 1 << 30 ? `${(n / (1 << 30)).toFixed(1)} GB` : n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(n / 1024)} KB`)
