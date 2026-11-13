<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { tableCell, tableHead, tableRow } from '@/components/admin/table'
  import { useSavedCue } from '@/components/admin/saved-cue'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { cn } from '@/lib/cn'
  import { getJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'

  interface OutreachData {
    config: { enabled: boolean; intervalMinutes: number; dailyDmCap: number; dailyDmTotalCap: number }
    agents: Array<{ model: string; displayName: string; proactive: boolean; personal: boolean }>
    events: Array<{ agentModel: string; kind: string; note: string | null; createdAt: string }>
  }

  // Proactive outreach (#59): opt-in periodic check-ins where agents surface
  // things through their own governed tools, plus agent-initiated DM caps.
  const qc = useQueryClient()
  const autosave = useSavedCue()
  const query = createQuery(() => ({
    queryKey: ['admin-outreach'],
    queryFn: (): Promise<OutreachData> => getJson<OutreachData>('/api/admin/outreach'),
    refetchInterval: 30_000,
  }))
  const data = $derived(query.data)
  const cfg = $derived(data?.config)
  const agents = $derived(data?.agents ?? [])
  const save = async (patch: Partial<OutreachData['config']>, proactiveAgents?: string[]) => {
    if (!cfg) return
    const body = {
      ...cfg,
      ...patch,
      proactiveAgents: proactiveAgents ?? agents.filter((a) => a.proactive).map((a) => a.model),
    }
    try {
      await putJson<{ ok: true }>('/api/admin/outreach', body)
      autosave.flash()
    } catch (e) {
      toastError('Save failed', e)
    }
    // Refetch even on failure: the route saves config and per-agent flags in
    // two steps, so a rejected PUT may still have changed half of it.
    await qc.invalidateQueries({ queryKey: ['admin-outreach'] })
  }
  let showEvents = $state(false)
  const unit = 'w-14 font-sans text-[13px] leading-5 text-muted'
  const KIND_LABEL: Record<string, string> = { dm: 'Message', sweep: 'Check-in' }
  const kindLabel = (kind: string) => KIND_LABEL[kind] ?? kind.charAt(0).toUpperCase() + kind.slice(1)
</script>

<SettingsSection
  title="Agent check-ins"
  description="Let agents look at their stalled work on a schedule and reach out about it."
  info="An agent you turn on gets a regular look at its stale or blocked work, and may act through its normal tools: a ticket comment, a channel post, or a direct message to your inbox. Everything it does stays attributed to it and follows each board's rules."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load outreach settings" errorVariant="compact">
      {#snippet skeleton()}
        <!-- Controls, agents and the recent list all seed from the query —
             hold their footprint so the toggle never flashes off. -->
        <div class="px-5 py-4"><SkeletonRows rows={6} /></div>
      {/snippet}
      {#snippet children(d)}
        <!-- QueryState's error branch holds a failed read: a toggle that is off
             and an empty agent list are this component's defaults, not the
             org's actual policy. -->
        <SettingsRow label="Scheduled check-ins" hint="The agents turned on below check in on their own.">
          <Toggle checked={cfg?.enabled ?? false} onChange={(checked) => void save({ enabled: checked })} title="Enable scheduled check-ins" />
        </SettingsRow>
        <SettingsRow label="Time between check-ins" hint="The shortest gap between one agent's check-ins." as="label">
          <Input
            size="sm" type="number" min={15} max={1440} class="w-24 tabular-nums"
            value={cfg?.intervalMinutes ?? 240}
            disabled={!cfg?.enabled}
            oninput={(e) => void save({ intervalMinutes: Math.max(15, Math.min(1440, Number(e.currentTarget.value) || 240)) })}
          />
          <span class={unit}>minutes</span>
        </SettingsRow>
        <SettingsRow label="Messages per person" hint="Direct messages an agent may start with one person each day." as="label">
          <Input
            size="sm" type="number" min={1} max={20} class="w-24 tabular-nums"
            value={cfg?.dailyDmCap ?? 3}
            oninput={(e) => void save({ dailyDmCap: Math.max(1, Math.min(20, Number(e.currentTarget.value) || 3)) })}
          />
          <span class={unit}>a day</span>
        </SettingsRow>
        <SettingsRow label="Messages per agent" hint="Direct messages one agent may start each day, everyone counted." as="label">
          <Input
            size="sm" type="number" min={1} max={500} class="w-24 tabular-nums"
            value={cfg?.dailyDmTotalCap ?? 20}
            oninput={(e) => void save({ dailyDmTotalCap: Math.max(1, Math.min(500, Number(e.currentTarget.value) || 20)) })}
          />
          <span class={unit}>a day</span>
        </SettingsRow>
        {#if agents.length > 0}
          <table class="w-full table-fixed">
            <colgroup>
              <col />
              <col class="w-28" />
            </colgroup>
            <thead>
              <tr>
                <th class={tableHead}>Agent</th>
                <th class={cn(tableHead, 'text-right')}>Checks in</th>
              </tr>
            </thead>
            <tbody>
              {#each agents as a (a.model)}
                <tr class={tableRow}>
                  <td class={tableCell}>
                    <span class="flex items-center gap-2">
                      <span class="truncate font-medium">{a.displayName}</span>
                      {#if a.personal}<Chip>Personal</Chip>{/if}
                    </span>
                  </td>
                  <td class={tableCell}>
                    <span class="flex justify-end">
                      <Toggle
                        checked={a.proactive}
                        title={`Let ${a.displayName} check in`}
                        onChange={(checked) => {
                          const next = agents.filter((x) => (x.model === a.model ? checked : x.proactive)).map((x) => x.model)
                          void save({}, next)
                        }}
                      />
                    </span>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
        {#if d.events.length > 0}
          <SettingsRow
            label="Recent outreach"
            hint="What agents did at their last check-ins."
            onclick={() => (showEvents = !showEvents)}
            expanded={showEvents}
          />
          {#if showEvents}
            <div transition:slide={{ duration: 150 }} class="max-h-[352px] overflow-y-auto">
              <table class="w-full table-fixed">
                <colgroup>
                  <col class="w-36" />
                  <col />
                  <col class="w-36" />
                  <col class="w-28" />
                </colgroup>
                <thead>
                  <tr>
                    <th class={tableHead}>Outcome</th>
                    <th class={tableHead}>Note</th>
                    <th class={tableHead}>Agent</th>
                    <th class={cn(tableHead, 'text-right')}>When</th>
                  </tr>
                </thead>
                <tbody>
                  {#each d.events as ev, i (i)}
                    <tr class={tableRow}>
                      <td class={tableCell}>{kindLabel(ev.kind)}</td>
                      <td class={cn(tableCell, 'truncate text-muted')} title={ev.note ?? ''}>{ev.note ?? '—'}</td>
                      <td class={cn(tableCell, 'truncate font-mono text-xs text-muted')}>{ev.agentModel}</td>
                      <td class={cn(tableCell, 'whitespace-nowrap text-right text-muted')}>{relativeTime(ev.createdAt)}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>
