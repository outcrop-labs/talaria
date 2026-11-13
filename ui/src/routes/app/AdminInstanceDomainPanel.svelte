<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Globe } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import RowStatus from '@/components/admin/RowStatus.svelte'

  /** The HOSTING domain — where this deployment lives. Verified by a self-
   *  fetch round trip (DNS + routing + TLS must land on this instance), and
   *  then used as the canonical base URL (stable OAuth callbacks, links). */
  const qc = useQueryClient()
  // `null` is a real answer here — no hosting domain configured yet, and the
  // form below invites you to set one. A FAILED read used to produce the same
  // null, so a blip showed a configured instance its "set a domain" form and a
  // Save would have rewritten live OAuth callbacks. Now only the server's own
  // `instance: null` gets there; a non-2xx becomes an error.
  type HostingDomain = { domain: string; verified: boolean; verifiedAt: string | null }

  const query = createQuery(() => ({
    queryKey: ['instance-domain'],
    queryFn: async (): Promise<HostingDomain | null> =>
      (await getJson<{ instance: HostingDomain | null }>('/api/admin/instance')).instance,
  }))
  let draft = $state('')
  let error = $state<string | null>(null)
  let busy = $state(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['instance-domain'] })

  const post = async (body: Record<string, unknown>) => {
    busy = true
    error = null
    try {
      // PUT writes the domain config; POST runs the verify action.
      const send = 'verify' in body ? postJson : putJson
      await send<{ ok: true }>('/api/admin/instance', body)
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }
</script>

<SettingsSection
  title="Web address"
  description="Where this Talaria is hosted."
  info="Checking the address confirms it reaches this exact instance. Once verified, links and sign-in callbacks all use it, so a Google or GitHub app needs only one callback address. This is separate from the email domains people sign up with."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load the web address" errorVariant="compact">
      {#snippet skeleton()}
        <SettingsRow label="Address"><Skeleton class="h-8 w-40" /></SettingsRow>
      {/snippet}
      {#snippet empty()}
        <!-- `null` is the server's own answer — no hosting domain configured yet
             — and this row invites you to set one. -->
        <SettingsRow label="Address" hint="The domain people open Talaria on." as="label">
          <Input
            size="sm"
            bind:value={draft}
            onkeydown={(e) => e.key === 'Enter' && draft.trim() && void post({ domain: draft.trim() })}
            placeholder="talaria.yourcompany.com"
            class="w-[280px] font-mono"
          />
          <Button size="md" variant="outline" disabled={!draft.trim() || busy} onclick={() => void post({ domain: draft.trim() })}>
            Set address
          </Button>
        </SettingsRow>
      {/snippet}
      {#snippet children(data)}
        <!-- `empty` owns the server's `null`, so children only ever sees a real
             domain. This is the narrowing the compiler needs. -->
        {#if data}
          <SettingsRow>
            {#snippet icon()}<Globe size={16} />{/snippet}
            {#snippet label()}<span class="font-mono text-[13px] font-normal">{data.domain}</span>{/snippet}
            {#if data.verified}
              <RowStatus status="ok" title="Requests to this address reach this instance">Verified</RowStatus>
            {:else}
              <RowStatus status="warn">Not verified</RowStatus>
              <Button size="sm" variant="outline" disabled={busy} onclick={() => void post({ verify: true })}>
                {busy ? 'Checking' : 'Verify'}
              </Button>
            {/if}
            <SettingsRowMenu
              label={`Actions for ${data.domain}`}
              items={[{ label: 'Remove address', danger: true, onSelect: () => void post({ domain: null }) }]}
            />
          </SettingsRow>
        {/if}
      {/snippet}
    </QueryState>
    {#if error}
      <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{error}</p>
    {/if}
  </SettingsGroup>
</SettingsSection>
