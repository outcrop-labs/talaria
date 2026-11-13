// The alerts the instance raises about itself, as the Overview and Alerts tabs
// read them. Same query key as the Manage overview, so the three share a cache.
import { createQuery } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'
import { NEUTRAL_DOT, type Dot } from './grammar'

export type AlertSeverity = 'critical' | 'warning' | 'info'

export interface Alert {
  severity: AlertSeverity
  title: string
  detail?: string
  href?: string
}

export const SEVERITY: Record<AlertSeverity, { label: string; dot: Dot }> = {
  critical: { label: 'Critical', dot: { status: 'danger' } },
  warning: { label: 'Warning', dot: { status: 'warn' } },
  info: { label: 'Info', dot: NEUTRAL_DOT },
}

export const useAlerts = () =>
  createQuery(() => ({
    queryKey: ['alerts'],
    queryFn: (): Promise<Alert[]> => getList<Alert>('/api/alerts', 'alerts'),
    refetchInterval: 60_000,
  }))
