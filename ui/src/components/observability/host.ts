// The machine the api can see. Shared by the Host tab and Overview's doorway.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface HostMount {
  mount: string
  source: string
  fstype: string
  total: number
  used: number
  available: number
  percent: number
  pressure: 'ok' | 'warn' | 'danger'
}

export interface HostProcess {
  pid: number
  name: string
  cpuPercent: number
  rss: number
}

export interface HostSnapshot {
  available: boolean
  sampledAt: number
  note: string | null
  cpu: { percent: number; iowaitPercent: number; cores: number } | null
  load: { one: number; five: number; fifteen: number } | null
  memory: { total: number; available: number; used: number; percent: number } | null
  swap: { present: boolean; total: number; used: number; percent: number } | null
  mounts: HostMount[]
  processes: HostProcess[]
}

export const useHost = () =>
  createQuery(() => ({
    queryKey: ['host'],
    queryFn: async (): Promise<HostSnapshot> => (await getJson<{ host: HostSnapshot }>('/api/host')).host,
    // CPU is a delta across samples. Five seconds is long enough to be a
    // real average and short enough that a filling disk shows up.
    refetchInterval: 5_000,
  }))
