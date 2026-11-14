<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getList, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import { sansFill } from '@/lib/ui-voice'

  interface OrgDomainRow {
    id: string
    domain: string
    verified: boolean
    verificationToken: string
    verifiedAt: string | null
  }

  /** Sign-up domains: prove a domain with a DNS TXT record and anyone signing
   *  in through Google with an email on it joins as a member — no invites. */
  const qc = useQueryClient()
  // An empty list here says "nobody can self-join". Only the server may say
  // that: a failed read used to hide a VERIFIED domain, and the panel then
  // invited the admin to add one that already exists.
  const query = createQuery(() => ({
    queryKey: ['org-domains'],
    queryFn: (): Promise<OrgDomainRow[]> => getList<OrgDomainRow>('/api/admin/domains', 'domains'),
  }))
  let draft = $state('')
  let error = $state<string | null>(null)
  let verifying = $state<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['org-domains'] })

  const add = async () => {
    if (!draft.trim()) return
    error = null
    try {
      await postJson<{ ok: true }>('/api/admin/domains', { domain: draft.trim() })
    } catch (e) {
      error = errorMessage(e)
      return
    }
    draft = ''
    await refresh()
  }
  const verify = async (id: string) => {
    verifying = id
    error = null
    try {
      const j = await postJson<{ verified: boolean; error?: string }>('/api/admin/domains', { verifyId: id })
      // An unverified answer is a 200: the DNS record just isn't there yet.
      if (!j.verified) error = j.error ?? 'verification failed'
    } catch (e) {
      error = errorMessage(e)
    }
    verifying = null
    await refresh()
  }
  const remove = async (d: OrgDomainRow) => {
    if (!(await confirm({ title: 'Remove domain', message: `Remove ${d.domain}? New self-joins stop; existing members keep their accounts.`, confirmLabel: 'Remove' }))) return
    try {
      await delJson<{ ok: true }>('/api/admin/domains', { id: d.id })
    } catch (e) {
      error = errorMessage(e)
    }
    await refresh()
  }
</script>

<SettingsSection
  title="Sign-up domains"
  description="Anyone who signs in with Google on a verified email domain joins as a member."
  info="This is the part after the @ in your team's email addresses, not where Talaria is hosted. Subdomains count separately, so add each one you use. People with a password sign-in are added on the People tab."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load your sign-up domains" errorVariant="compact">
      {#snippet skeleton()}<div class="px-5 py-4"><SkeletonRows rows={2} /></div>{/snippet}
      {#snippet children(data)}
        <!-- No `?? []` fallback: the missing-data case forked to QueryState's
             error branch, so reaching here means the read landed. -->
        {#each data as d (d.id)}
          <div>
            <SettingsRow>
              {#snippet label()}<span class="font-mono text-[13px] font-normal">{d.domain}</span>{/snippet}
              {#snippet status()}
                {#if d.verified}
                  <RowStatus status="ok" title="People on this domain can join on their own">Verified</RowStatus>
                {:else}
                  <RowStatus status="warn">Waiting for DNS</RowStatus>
                {/if}
              {/snippet}
              {#if !d.verified}
                <Button size="sm" variant="outline" disabled={verifying === d.id} onclick={() => void verify(d.id)}>
                  {verifying === d.id ? 'Checking' : 'Verify'}
                </Button>
              {/if}
              <SettingsRowMenu
                label={`Actions for ${d.domain}`}
                items={[{ label: 'Remove domain', danger: true, onSelect: () => void remove(d) }]}
              />
            </SettingsRow>
            {#if !d.verified}
              <div class="flex flex-wrap items-center gap-x-2 gap-y-1 border-t border-line-subtle px-5 py-3 font-sans text-[13px] leading-5 text-muted">
                <span>Add a TXT record on</span>
                <code class="font-mono text-xs text-fg">_talaria-verify.{d.domain}</code>
                <span>with the value</span>
                <code class="min-w-0 truncate rounded {sansFill} px-1.5 py-0.5 font-mono text-xs text-fg">{d.verificationToken}</code>
                <CopyButton value={d.verificationToken} title="Copy the TXT value" />
                <span>then press Verify.</span>
              </div>
            {/if}
          </div>
        {/each}
        <SettingsRow
          label={data.length === 0 ? 'No domains yet' : 'Add a domain'}
          hint="The part after the @ in your team's email addresses."
          as="label"
          apart
        >
          <Input
            size="sm"
            bind:value={draft}
            onkeydown={(e) => e.key === 'Enter' && void add()}
            placeholder="yourcompany.com"
            class="w-[280px] font-mono"
          />
          <Button size="md" variant="outline" onclick={() => void add()} disabled={!draft.trim()}>Add domain</Button>
        </SettingsRow>
        {#if error}
          <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{error}</p>
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>
