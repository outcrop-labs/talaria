<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
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

  const save = async (row: string, patch: { enabled?: boolean; services?: string[] }) => {
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
    autosave.flash(row)
  }

  const toggleService = (id: string, on: boolean) => {
    const current = data?.permitted ?? []
    void save(id, { services: on ? [...current, id] : current.filter((s) => s !== id) })
  }

  // The header's captions and each row's cells share these widths.
  const FLOW = 'w-48 shrink-0'
  const SWITCH = 'flex w-9 shrink-0 justify-end'

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
        <SettingsRow label="Allow sign-in" hint="Agents may sign in to the services you turn on below." saved={autosave.saved('enabled')}>
          <Toggle checked={d.enabled} onChange={(on) => void save('enabled', { enabled: on })} title="Allow agents to sign in to coding accounts" />
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
            <SettingsHeaderRow label="Service">
              <span class={FLOW}>Sign-in</span>
              <span class={SWITCH}></span>
            </SettingsHeaderRow>
            {#each d.services as s (s.id)}
              <SettingsRow saved={autosave.saved(s.id)} class="min-h-11 py-1.5">
                {#snippet label()}<span class="text-[13px]">{s.name}</span>{/snippet}
                <!-- The switch says allowed or off. Only a service the
                     installed tool dropped needs a word of its own. -->
                {#snippet meta()}
                  {#if s.unavailable}
                    <RowStatus status="warn" title="Allowed here, but the installed coding tool no longer offers it">No longer offered</RowStatus>
                  {/if}
                {/snippet}
                <span class="{FLOW} font-sans text-[13px] leading-5 text-muted">{flowLabel(s.flow)}</span>
                <span class={SWITCH}>
                  <Toggle checked={s.permitted} onChange={(on) => toggleService(s.id, on)} title={`Allow ${s.name}`} />
                </span>
              </SettingsRow>
            {/each}
          {/if}
          {#if d.permitted.length === 0}
            <NoticeRow status="idle">No service is allowed yet, so no agent can sign in.</NoticeRow>
          {/if}
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>
