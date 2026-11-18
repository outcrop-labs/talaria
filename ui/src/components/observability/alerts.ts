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

/** `action` names the way out of the row: something broken is fixed, something
 *  worth knowing is reviewed. */
export const SEVERITY: Record<AlertSeverity, { label: string; dot: DotStatus; action: string }> = {
  critical: { label: 'Critical', dot: 'danger', action: 'Fix' },
  warning: { label: 'Warning', dot: 'warn', action: 'Fix' },
  info: { label: 'Info', dot: 'idle', action: 'Review' },
}

export const useAlerts = () =>
  createQuery(() => ({
    queryKey: ['alerts'],
    queryFn: (): Promise<Alert[]> => getList<Alert>('/api/alerts', 'alerts'),
    refetchInterval: 60_000,
  }))
