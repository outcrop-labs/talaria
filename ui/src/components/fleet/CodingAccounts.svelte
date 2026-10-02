<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, getJsonOr404, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import CodingLoginModal from './CodingLoginModal.svelte'
  import type { CodingAccountsBody, CodingModel, CodingService } from './coding-accounts'
  import { loadModelsInto, roleLabel } from './coding-accounts'

  // This agent's coding accounts: the subscriptions its harness may run on,
  // which plan runs a job by default, and which model fills each of omp's
  // roles on each plan.
  //
  // THE GATEWAY IS A PLAN HERE. It is listed alongside the subscriptions, can
  // be made the default, and has its own role picks — so signing a
  // subscription in never takes away the ability to say "run this through the
  // gateway, on the org's models".
  //
  // The panel renders NOTHING when the feature is off: its routes answer 404
  // in that state (a feature nobody enabled should look absent, not
  // forbidden), and `getJsonOr404` turns that into a null rather than an error
  // banner on an agent's summary tab.
  let { agentId }: { agentId: string } = $props()

  const qc = useQueryClient()
  const accounts = createQuery(() => ({
    queryKey: ['coding-accounts', agentId],
    queryFn: (): Promise<CodingAccountsBody | null> =>
      getJsonOr404<CodingAccountsBody>(`/api/workbench/coding/accounts/${agentId}`),
  }))
  const services = createQuery(() => ({
    queryKey: ['coding-services'],
    queryFn: (): Promise<{ services: CodingService[]; permitted: string[] } | null> =>
      getJsonOr404<{ services: CodingService[]; permitted: string[] }>('/api/workbench/coding/services'),
  }))

  /** Model lists are per provider and only needed once a plan is expanded. */
  let expanded = $state<string | null>(null)
  let models = $state<Record<string, CodingModel[]>>({})

  let signingIn = $state<CodingService | null>(null)
  let busy = $state(false)

  const reload = () => qc.invalidateQueries({ queryKey: ['coding-accounts', agentId] })

  const setDefault = async (accountId: number | null) => {
    busy = true
    try {
      await putJson(`/api/workbench/coding/accounts/${agentId}`, { action: 'default', accountId })
      await reload()
    } catch (e) {
      toastError('Could not change the default plan', e)
    } finally {
      busy = false
    }
  }

  const setRole = async (accountId: number | null, roles: Record<string, string | null>) => {
    busy = true
    try {
      await putJson(`/api/workbench/coding/accounts/${agentId}`, { action: 'roles', accountId, roles })
      await reload()
    } catch (e) {
      toastError('Could not set that model', e)
    } finally {
      busy = false
    }
  }

  const signOut = async (id: number, provider: string) => {
    const ok = await confirm({
      title: `Sign out of ${provider}?`,
      message: `This deletes the credential from this instance. Tickets pinned to it fall back to the agent's default plan, and signing back in means going through ${provider} again.`,
      confirmLabel: 'Sign out',
      danger: true,
    })
    if (!ok) return
    busy = true
    try {
      await delJson(`/api/workbench/coding/accounts/${agentId}`, { accountId: id })
      pushToast({ title: 'Signed out', tone: 'success' })
      await reload()
    } catch (e) {
      toastError('Could not sign out', e)
    } finally {
      busy = false
    }
  }

  /** Services with nothing signed in yet, which the org permits. */
  const offerable = $derived(
    (services.data?.services ?? []).filter(
      (s) =>
        s.permitted &&
        s.available !== false &&
        !(accounts.data?.accounts ?? []).some((a) => a.provider === s.storeAs),
    ),
  )
</script>

<QueryState query={accounts} errorTitle="Could not load coding accounts" errorVariant="inline">
  {#snippet skeleton()}<div class="mt-4"><SkeletonRows rows={2} /></div>{/snippet}
  {#snippet children(d)}
    {#if d}
      <div class="mt-4 space-y-1.5">
        <div class="flex items-center gap-1.5">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Coding accounts</span>
          <InfoTip
            text="Sign this agent in to a coding subscription (Claude Pro/Max, ChatGPT Codex, Copilot, Gemini…) and its harness runs on that account instead of the org's Talaria gateway. One account per service. The agent itself keeps the model its settings configure — this only moves what the coding harness runs on. Spend on a subscription lands on that provider's bill, not Talaria's ledger."
          />
        </div>

        {#each [{ accountId: null as number | null, provider: 'talaria', label: 'Talaria gateway', gateway: true, roles: d.gatewayRoles ?? {}, account: null }, ...d.accounts.map((a) => ({ accountId: a.id as number | null, provider: a.provider, label: a.email ?? a.orgName ?? a.provider, gateway: false, roles: a.roles ?? {}, account: a }))] as plan (plan.accountId ?? 'gateway')}
          {@const isDefault = plan.gateway
            ? !d.accounts.some((a) => a.primary)
            : (plan.account?.primary ?? false)}
          <div class="rounded-md border border-line-subtle bg-raised/40 px-2 py-1.5">
            <div class="flex flex-wrap items-center gap-1.5">
              <span class="min-w-0 flex-1 truncate text-xs text-fg">{plan.label}</span>
              <Chip tone="neutral" class="px-2 py-0.5 normal-case">{plan.provider}</Chip>
              {#if isDefault}<Chip tone="accent" class="px-2 py-0.5 normal-case">default</Chip>{/if}
              {#if plan.account?.disabledAt}
                <Chip tone="danger" class="px-2 py-0.5 normal-case">needs sign-in</Chip>
              {/if}
              {#if plan.account && !plan.account.permitted}
                <Chip tone="warn" class="px-2 py-0.5 normal-case">not permitted</Chip>
              {/if}
              {#if !isDefault}
                <Button size="xs" variant="ghost" disabled={busy} onclick={() => void setDefault(plan.accountId)}>
                  Make default
                </Button>
              {/if}
              <Button
                size="xs"
                variant="ghost"
                onclick={() => {
                  const k = String(plan.accountId)
                  expanded = expanded === k ? null : k
                  if (expanded) void loadModelsInto(models, plan.provider)
                }}
              >
                Models
              </Button>
              {#if plan.account}
                <Button
                  size="xs"
                  variant="ghost"
                  disabled={busy}
                  onclick={() => void signOut(plan.account.id, plan.account.provider)}
                >
                  Sign out
                </Button>
              {/if}
            </div>

            {#if plan.account?.disabledAt && plan.account.disabledCause}
              <p class="mt-1 text-xs text-danger">{plan.account.disabledCause}</p>
            {/if}

            {#if expanded === String(plan.accountId)}
              <div class="mt-1.5 space-y-1" transition:slide={{ duration: 150 }}>
                <p class="text-xs text-muted">
                  Which model fills each harness role on this plan. Blank falls back to this plan's default, then to
                  the org's Workbench model roles.
                </p>
                {#each d.roles as role (role)}
                  <label class="flex items-center gap-1.5">
                    <span class="w-16 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                      {roleLabel(role)}
                    </span>
                    <Select
                      size="sm"
                      class="w-56"
                      value={plan.roles[role] ?? ''}
                      disabled={busy}
                      onchange={(e) => {
                        const v = (e.currentTarget as HTMLSelectElement).value
                        void setRole(plan.accountId, { ...plan.roles, [role]: v === '' ? null : v })
                      }}
                    >
                      <option value="">— falls back —</option>
                      {#each models[plan.provider] ?? [] as m (m.id)}
                        <option value={m.id}>{m.name ?? m.id}</option>
                      {/each}
                    </Select>
                  </label>
                {/each}
              </div>
            {/if}
          </div>
        {/each}

        {#if d.resolved}
          <p class="text-xs text-muted">
            A job with no ticket pin runs on
            <span class="text-fg">{d.resolved.gateway ? 'the Talaria gateway' : d.resolved.provider}</span>
            {#if d.resolved.models?.default}· <span class="font-mono">{d.resolved.models.default}</span>{/if}
          </p>
        {:else}
          <p class="text-xs text-muted">
            Nothing configured, so the harness runs on the org's Workbench model roles.
          </p>
        {/if}

        {#if offerable.length > 0}
          <div class="flex flex-wrap items-center gap-1.5 pt-0.5">
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Sign in to</span>
            {#each offerable as s (s.id)}
              <Button size="xs" variant="ghost" onclick={() => (signingIn = s)}>{s.name}</Button>
            {/each}
          </div>
        {:else if (services.data?.permitted?.length ?? 0) === 0}
          <p class="text-xs text-muted">No services permitted yet — an admin allows them in Admin → Coding accounts.</p>
        {/if}
      </div>
    {/if}
  {/snippet}
</QueryState>

{#if signingIn}
  <CodingLoginModal
    {agentId}
    service={signingIn}
    onClose={() => (signingIn = null)}
    onDone={async () => {
      signingIn = null
      await reload()
    }}
  />
{/if}
