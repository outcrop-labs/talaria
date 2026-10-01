<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
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
  const savedFlash = useSavedFlash()

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
    savedFlash.flash()
  }

  const toggleService = (id: string, on: boolean) => {
    const current = data?.permitted ?? []
    void save({ services: on ? [...current, id] : current.filter((s) => s !== id) })
  }

  /** The flow shape, in words an admin reads. */
  const flowLabel = (flow: string | null): string => {
    switch (flow) {
      case 'oauth-code':
        return 'browser sign-in'
      case 'device-code':
        return 'device code'
      case 'custom':
        return 'provider-specific'
      default:
        return 'sign-in'
    }
  }
</script>

<Panel>
  <SectionHeader
    class="mb-4"
    title="Coding accounts"
    info="Lets a Developer Agent's coding harness run on a signed-in coding subscription (Claude Pro/Max, ChatGPT Codex, GitHub Copilot, Gemini…) instead of this org's Talaria gateway. One account per service, per agent, signed in by whoever can edit the agent; a ticket can pin a different one when a plan runs out. Credentials are sealed in this instance's database and never reach an agent's config. Note that harness spend on a subscription does not pass through the gateway, so it does not appear in Talaria's ledger — the provider's own dashboard is the record. The agent's own model is unaffected."
  />
  <QueryState query={query} errorTitle="Could not load the coding-account policy" errorVariant="compact">
    {#snippet skeleton()}
      <div class="flex flex-wrap items-center gap-3">
        <Skeleton class="h-4 w-4" />
        <Skeleton class="h-3 w-56 rounded-full" />
      </div>
    {/snippet}
    {#snippet children(d)}
      <div class="space-y-3">
        <Checkbox
          checked={d.enabled}
          onChange={(on) => void save({ enabled: on })}
          label="Allow agents to sign in to coding accounts"
        />

        {#if d.enabled}
          <div class="space-y-1.5">
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              Services this organization permits
            </span>
            {#if d.rosterError}
              <p class="text-xs text-warning">
                The sign-in roster could not be read ({d.rosterError}). The services already permitted keep working;
                this list fills in once the omp auth bridge is reachable.
              </p>
            {/if}
            {#if d.services.length === 0 && !d.rosterError}
              <p class="text-xs text-muted">No services available.</p>
            {:else}
              <div class="flex max-h-48 flex-wrap gap-1.5 overflow-y-auto">
                {#each d.services as s (s.id)}
                  <Chip
                    onSelect={() => toggleService(s.id, !s.permitted)}
                    selected={s.permitted}
                    class="px-2.5 py-0.5 normal-case"
                    title={s.unavailable
                      ? 'Permitted, but the installed omp no longer offers it'
                      : flowLabel(s.flow)}
                  >
                    {s.name}{#if s.unavailable}&nbsp;·&nbsp;unavailable{/if}
                  </Chip>
                {/each}
              </div>
            {/if}
            {#if d.permitted.length === 0}
              <p class="text-xs text-muted">
                Nothing permitted yet, so no agent can sign in. Pick the services this organization allows.
              </p>
            {/if}
            <p class="text-xs text-muted">
              Removing a service stops its accounts driving any harness immediately — jobs fall back to the gateway —
              but keeps the credentials, so re-permitting it does not need a fresh sign-in.
            </p>
          </div>
        {/if}
        {#if savedFlash.saved}<p class="text-xs text-success">Saved</p>{/if}
      </div>
    {/snippet}
  </QueryState>
</Panel>
