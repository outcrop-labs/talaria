<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { delJson, getJsonOr404, putJson } from '@/lib/fetch-json'
  import { loadModelsInto, type CodingModel } from '@/components/fleet/coding-accounts'
  import { toastError } from '@/lib/toast.svelte'

  // Which coding plan THIS ticket's work runs on.
  //
  // WHY A TICKET NEEDS ITS OWN ANSWER. A subscription runs out. Somebody is
  // halfway through a ticket when the plan behind it hits its weekly cap, and
  // what they want is to move this ticket onto the other account — not to
  // re-point the agent and every other ticket with it. So the pick lives here,
  // outranks the agent's default, and names the model as well as the plan: the
  // other account's flagship is not always what the agent's default role would
  // have chosen.
  //
  // Renders nothing when there is nothing to choose: the feature off (the route
  // 404s), no agent assigned, or a single plan with no alternative.
  let { taskId, canEdit }: { taskId: string; canEdit: boolean } = $props()

  interface Plan {
    accountId: number | null
    provider: string
    label: string
    gateway: boolean
    roles?: Record<string, string>
  }
  interface PinBody {
    pin: { accountId: number | null; provider: string; email: string | null; model: string | null } | null
    plans: Plan[]
    resolved: {
      accountId: number | null
      provider: string
      source: 'ticket' | 'primary'
      gateway: boolean
      models: { role: string; model: string }[]
    } | null
  }

  const qc = useQueryClient()
  const key = $derived(['coding-pin', taskId])
  const query = createQuery(() => ({
    queryKey: ['coding-pin', taskId],
    queryFn: (): Promise<PinBody | null> => getJsonOr404<PinBody>(`/api/workbench/coding/pin/${taskId}`),
  }))
  const data = $derived(query.data)

  /** The models of whichever plan is selected, for the model picker. */
  let models = $state<Record<string, CodingModel[]>>({})

  let busy = $state(false)
  /** Which plan the controls show: the pin, else what resolution picked. */
  const selected = $derived.by(() => {
    if (!data) return null
    if (data.pin) return data.pin.accountId
    return data.resolved?.accountId ?? null
  })
  const pinnedModel = $derived(data?.pin?.model ?? '')

  const pin = async (accountId: number | null, model: string | null) => {
    busy = true
    try {
      await putJson(`/api/workbench/coding/pin/${taskId}`, { accountId, model })
      await qc.invalidateQueries({ queryKey: key })
    } catch (e) {
      toastError('Could not change this ticket’s coding plan', e)
    } finally {
      busy = false
    }
  }

  const unpin = async () => {
    busy = true
    try {
      await delJson(`/api/workbench/coding/pin/${taskId}`)
      await qc.invalidateQueries({ queryKey: key })
    } catch (e) {
      toastError('Could not clear the pin', e)
    } finally {
      busy = false
    }
  }

  $effect(() => {
    const provider = data?.plans.find((pl) => pl.accountId === selected)?.provider
    if (provider) void loadModelsInto(models, provider)
  })

  // One plan is the gateway, which is always listed — so "nothing to choose"
  // means no account was ever signed in for this agent.
  const worthShowing = $derived((data?.plans.length ?? 0) > 1)
</script>

{#if data && worthShowing}
  <div class="flex flex-wrap items-center gap-1.5 text-xs">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Codes on</span>
    {#if canEdit}
      <Select
        size="sm"
        class="w-44"
        value={String(selected ?? '')}
        disabled={busy}
        onchange={(e) => {
          const raw = (e.currentTarget as HTMLSelectElement).value
          void pin(raw === '' ? null : Number(raw), null)
        }}
      >
        {#each data.plans as plan (plan.accountId ?? 'gateway')}
          <option value={String(plan.accountId ?? '')}>{plan.label}</option>
        {/each}
      </Select>
      <Select
        size="sm"
        class="w-52"
        value={pinnedModel}
        disabled={busy}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value
          void pin(selected, v === '' ? null : v)
        }}
      >
        <option value="">plan’s default model</option>
        {#each models[data.plans.find((pl) => pl.accountId === selected)?.provider ?? ''] ?? [] as m (m.id)}
          <option value={m.id}>{m.name ?? m.id}</option>
        {/each}
      </Select>
      {#if data.pin}
        <Button size="xs" variant="ghost" disabled={busy} onclick={() => void unpin()}>Use the agent’s default</Button>
      {/if}
    {:else if data.resolved}
      <span class="text-fg">{data.resolved.gateway ? 'the Talaria gateway' : data.resolved.provider}</span>
    {/if}
    {#if data.resolved}
      {@const def = data.resolved.models.find((m) => m.role === 'default')}
      <span class="text-muted">
        {data.resolved.source === 'ticket' ? 'pinned to this ticket' : 'the agent’s default'}
        {#if def}· <span class="font-mono">{def.model}</span>{/if}
      </span>
    {/if}
  </div>
{/if}
