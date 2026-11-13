// The alerts the instance raises about itself, as the Overview and Alerts tabs
// read them. Same query key as the Manage overview, so the three share a cache.
import { createQuery } from '@tanstack/svelte-query'
import type { DotStatus } from '@/components/ui/chip'
import { getList } from '@/lib/fetch-json'

export type AlertSeverity = 'critical' | 'warning' | 'info'

export interface Alert {
  severity: AlertSeverity
  title: string
  detail?: string
  href?: string
}

export const SEVERITY: Record<AlertSeverity, { label: string; dot: DotStatus }> = {
  critical: { label: 'Critical', dot: 'danger' },
  warning: { label: 'Warning', dot: 'warn' },
  info: { label: 'Info', dot: 'idle' },
}

export const useAlerts = () =>
  createQuery(() => ({
    queryKey: ['alerts'],
    queryFn: (): Promise<Alert[]> => getList<Alert>('/api/alerts', 'alerts'),
    refetchInterval: 60_000,
  }))
