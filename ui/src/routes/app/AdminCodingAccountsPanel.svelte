<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import { tableCell, tableHead, tableRow } from '@/components/admin/table'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import { cn } from '@/lib/cn'
  import { getJson, putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'

  // Coding accounts — whether a Developer Agent may run its harness on a
  // coding subscription instead of the org's gateway, and which services the
  // org allows.
  //
  // TWO CONTROLS ON PURPOSE. Turning the feature on permits nothing; the org
  // then says which services. "Developers may sign agents in to coding
  // subscriptions" and "to anything omp supports" are different decisions, and
  // an org that wants Copilot and Codex but not a personal Claude Max
  // subscription needs to be able to say exactly that.
  //
  // The roster is omp's own — a provider upstream adds appears here with no
  // Talaria change — so this panel can show a service it cannot describe
  // (`unavailable`) when an allowlisted one disappears from the installed omp.
  interface Service {
    id: string
    name: string
    flow: string | null
    permitted: boolean
    unavailable?: boolean
  }
  interface Body {
    enabled: boolean
    permitted: string[]
    services: Service[]
    rosterError: string | null
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['admin-coding-accounts'],
    queryFn: (): Promise<Body> => getJson<Body>('/api/admin/coding-accounts'),
  }))
  const data = $derived(query.data)
  const autosave = useAutosave()

  const save = async (patch: { enabled?: boolean; services?: string[] }) => {
    const body = {
      enabled: patch.enabled ?? data?.enabled ?? false,
      services: patch.services ?? data?.permitted ?? [],
    }
    try {
      await putJson<Body>('/api/admin/coding-accounts', body)
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['admin-coding-accounts'] })
    autosave.flash()
  }

  const toggleService = (id: string, on: boolean) => {
    const current = data?.permitted ?? []
    void save({ services: on ? [...current, id] : current.filter((s) => s !== id) })
  }

  /** The flow shape, in words an admin reads. */
  const flowLabel = (flow: string | null): string => {
    switch (flow) {
      case 'oauth-code':
        return 'Browser sign-in'
      case 'device-code':
        return 'Device code'
      case 'custom':
        return 'Provider-specific'
      default:
        return 'Sign-in'
    }
  }
</script>

<SettingsSection
  title="Coding subscriptions"
  description="Let a developer agent's coding tool run on a signed-in subscription instead of your gateway."
  info="One account per service, per agent, signed in by whoever can edit the agent. Credentials are stored encrypted on this instance and never reach an agent's config. Spend on a subscription does not pass through the gateway, so it is not in Talaria's ledger and the provider's own dashboard is the record. Turning a service off stops its accounts at once and jobs fall back to the gateway, but the credentials are kept, so turning it back on needs no new sign-in."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load the coding-account policy" errorVariant="compact">
      {#snippet skeleton()}
        <div class="px-5 py-4"><SkeletonRows rows={2} /></div>
      {/snippet}
      {#snippet children(d)}
        <SettingsRow label="Allow sign-in" hint="Agents may sign in to the services you turn on below.">
          <Toggle checked={d.enabled} onChange={(on) => void save({ enabled: on })} title="Allow agents to sign in to coding accounts" />
        </SettingsRow>

        {#if d.enabled}
          {#if d.rosterError}
            <NoticeRow status="warn">
              The list of services could not be read ({d.rosterError}). Services already allowed keep working.
            </NoticeRow>
          {/if}
          {#if d.services.length === 0 && !d.rosterError}
            <NoticeRow status="idle">No services available.</NoticeRow>
          {:else if d.services.length > 0}
            <table class="w-full table-fixed">
              <colgroup>
                <col />
                <col class="w-48" />
                <col class="w-[4.5rem]" />
              </colgroup>
              <thead>
                <tr>
                  <th class={tableHead}>Service</th>
                  <th class={tableHead}>Sign-in</th>
                  <th class={tableHead}><span class="sr-only">Allowed</span></th>
                </tr>
              </thead>
              <tbody>
                {#each d.services as s (s.id)}
                  <tr class={tableRow}>
                    <!-- The switch says allowed or off. Only a service the
                         installed tool dropped needs a word of its own. -->
                    <td class={tableCell}>
                      <span class="flex min-w-0 items-center gap-4">
                        <span class="truncate font-medium">{s.name}</span>
                        {#if s.unavailable}
                          <RowStatus status="warn" title="Allowed here, but the installed coding tool no longer offers it">No longer offered</RowStatus>
                        {/if}
                      </span>
                    </td>
                    <td class={cn(tableCell, 'text-muted')}>{flowLabel(s.flow)}</td>
                    <td class={tableCell}>
                      <Toggle class="justify-end" checked={s.permitted} onChange={(on) => toggleService(s.id, on)} title={`Allow ${s.name}`} />
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
          {#if d.permitted.length === 0}
            <NoticeRow status="idle">No service is allowed yet, so no agent can sign in.</NoticeRow>
          {/if}
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>
