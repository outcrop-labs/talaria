<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { getList } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

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
  const data = $derived(query.data)
  let draft = $state('')
  let error = $state<string | null>(null)
  let verifying = $state<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['org-domains'] })

  const add = async () => {
    if (!draft.trim()) return
    error = null
    const r = await fetch('/api/admin/domains', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ domain: draft.trim() }),
    })
    if (!r.ok) {
      error = ((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed'
      return
    }
    draft = ''
    await refresh()
  }
  const verify = async (id: string) => {
    verifying = id
    error = null
    const r = await fetch('/api/admin/domains', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ verifyId: id }),
    })
    const j = (await r.json().catch(() => ({}))) as { verified?: boolean; error?: string }
    verifying = null
    if (!j.verified) error = j.error ?? 'verification failed'
    await refresh()
  }
</script>

<Panel class="mt-4">
  <SectionHeader
    title="Email sign-up domains"
    info="The domain after the @ in your team's EMAIL addresses — not where Talaria is hosted (talaria.yourcompany.com hosting still means yourcompany.com emails). Add it, publish the TXT record to prove ownership, and anyone signing in with Google on that email domain becomes a member automatically — no invites, no env edits. Email subdomains are separate; add each you use. Password logins stay env-managed."
  />
  {#if query.isPending}
    <SkeletonRows rows={2} />
  {:else if !data}
    <QueryError
      variant="inline"
      error={query.error}
      title="Could not load your sign-up domains"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="space-y-2">
      {#if data.length === 0}
        <EmptyState
          variant="inline"
          title="No email domains yet"
          hint="add the domain your team's email addresses use to open self-service joins."
          class="font-sans"
        />
      {/if}
      <!-- No `?? []` fallback: `!data` already forked to the error above, so
           reaching here means the read landed. -->
      {#each data as d (d.id)}
        <div class="space-y-1.5 rounded-md border border-line p-2.5">
          <div class="flex items-center gap-2">
            <span class="font-mono text-[13px] text-fg">{d.domain}</span>
            {#if d.verified}
              <span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-success">
                <span aria-hidden="true" class="h-1.5 w-1.5 shrink-0 rounded-full bg-success"></span>
                verified — self-joins open
              </span>
            {:else}
              <span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-warning">
                <span aria-hidden="true" class="h-1.5 w-1.5 shrink-0 rounded-full bg-warning"></span>
                awaiting DNS proof
              </span>
            {/if}
            <span class="flex-1"></span>
            {#if !d.verified}
              <Button size="sm" variant="outline" disabled={verifying === d.id} onclick={() => void verify(d.id)}>
                {verifying === d.id ? 'Checking' : 'Verify'}
              </Button>
            {/if}
            <button
              type="button"
              title="Remove — self-joins from this domain stop immediately"
              onclick={async () => {
                if (!(await confirm({ title: 'Remove domain', message: `Remove ${d.domain}? New self-joins stop; existing members keep their accounts.`, confirmLabel: 'Remove' }))) return
                await fetch('/api/admin/domains', {
                  method: 'DELETE',
                  credentials: 'same-origin',
                  headers: { 'content-type': 'application/json' },
                  body: JSON.stringify({ id: d.id }),
                })
                await refresh()
              }}
              class="text-muted transition-colors hover:text-danger"
            >
              <Trash2 size={13} />
            </button>
          </div>
          {#if !d.verified}
            <div class="flex items-center gap-2">
              <span class="shrink-0 text-[11px] text-muted">TXT on _talaria-verify.{d.domain}:</span>
              <code class="min-w-0 flex-1 truncate rounded bg-raised px-1.5 py-0.5 font-mono text-[11px] text-fg">{d.verificationToken}</code>
              <CopyButton value={d.verificationToken} label="Copy" class="text-xs" />
            </div>
          {/if}
        </div>
      {/each}
      <div class="flex items-center gap-2 pt-1">
        <Input
          size="sm"
          bind:value={draft}
          onkeydown={(e) => e.key === 'Enter' && void add()}
          placeholder="yourcompany.com — your email domain"
          class="w-72"
        />
        <Button size="sm" onclick={() => void add()} disabled={!draft.trim()}>
          Add domain
        </Button>
      </div>
      {#if error}
        <div transition:slide={{ duration: 150 }} class="text-xs text-danger">
          {error}
        </div>
      {/if}
    </div>
  {/if}
</Panel>
