// The instance's display name, read from the PUBLIC identity beacon — the
// same read works pre-login, so the sign-in tab is branded too. The name is
// admin-set (AdminCompanyNamePanel → app_settings); what the tab does with
// it is tabTitle() over in tab-title.ts.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

/** The beacon's wire shape — the instance id plus the display name, null
 *  when the operator hasn't named the instance. */
export interface InstanceBranding {
  instance: string
  companyName: string | null
}

/** The boot read behind the tab title. Long staleTime: the name changes
 *  rarely, and the admin panel invalidates this key on save. */
export function useInstanceBranding() {
  return createQuery(() => ({
    queryKey: ['instance-branding'],
    queryFn: (): Promise<InstanceBranding> =>
      getJson<InstanceBranding>('/api/well-known/talaria-instance'),
    staleTime: 5 * 60_000,
  }))
}
