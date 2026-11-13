<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { CalendarClock } from '@lucide/svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { errorMessage, getList, postJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { listStagger, slide } from '@/lib/motion'
  import Button from '@/components/ui/Button.svelte'
  import { type CronFormHandle, type CronJob, useCronMutations } from './agent-crons'
  import CronForm from './CronForm.svelte'
  import CronListSkeleton from './CronListSkeleton.svelte'
  import CronRow from './CronRow.svelte'

  interface FleetCronAgent {
    id: string
    slug: string
    displayName: string
    jobs: CronJob[]
    error?: string
  }

  // Fleet-wide schedules (admin): every agent's jobs + create-across-agents.
  // The third tab of AGENTS. It was a takeover modal behind a toolbar icon,
  // which is an affordance for one action rather than for a surface you come
  // back to — and schedules are something you review, not something you fire.

  /** The New schedule dialog. The page header owns the button that opens it. */
  let { composing = $bindable(false) }: { composing?: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['fleet-crons'],
    queryFn: (): Promise<FleetCronAgent[]> => getList<FleetCronAgent>('/api/fleet/crons', 'agents'),
  }))
  const agents = $derived(query.data ?? [])
  let selected = $state<Set<string> | null>(null) // null = all
  let busy = $state(false)
  let form = $state<CronFormHandle | null>(null)
  let summary = $state<string | null>(null)
  const chosen = $derived(selected ?? new Set(agents.map((a) => a.id)))

  const toggle = (id: string) => {
    const next = new Set(chosen)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    selected = next
  }

  const create = async (input: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    busy = true
    summary = null
    try {
      const j = await postJson<{ results?: Array<{ ok: boolean; error?: string }> }>('/api/fleet/crons', {
        ...input,
        agentIds: [...chosen],
      })
      if (!j.results) {
        summary = 'Could not create the schedule.'
        return false
      }
      const failed = j.results.filter((x) => !x.ok)
      summary =
        failed.length === 0
          ? `Created on ${j.results.length} agent${j.results.length === 1 ? '' : 's'}.`
          : `Created on ${j.results.length - failed.length}, failed on ${failed.length}: ${failed[0]?.error ?? ''}`
      await qc.invalidateQueries({ queryKey: ['fleet-crons'] })
      if (failed.length === 0) composing = false
      return failed.length === 0
    } catch (e) {
      summary = errorMessage(e)
      return false
    } finally {
      busy = false
    }
  }

  const { edit, act } = useCronMutations({
    // Row actions are fire-and-forget here (CronRow doesn't render an error
    // slot in the fleet tab) — a toast is where a failed one gets said.
    onError: (e, what) =>
      toastError(what === 'edit' ? 'Save failed' : `${what.charAt(0).toUpperCase()}${what.slice(1)} failed`, e),
    invalidate: () => qc.invalidateQueries({ queryKey: ['fleet-crons'] }),
  })

  const withJobs = $derived(agents.flatMap((a) => a.jobs.map((j) => ({ agent: a, job: j }))))
  const unreachable = $derived(agents.filter((a) => a.error).map((a) => a.displayName))
</script>

{#snippet calendarIcon()}<CalendarClock size={22} />{/snippet}

<div class="space-y-4">
  <QueryState {query} errorTitle="Could not load schedules" errorVariant="compact" isEmpty={() => withJobs.length === 0}>
    {#snippet skeleton()}<CronListSkeleton />{/snippet}
    {#snippet empty()}<EmptyState icon={calendarIcon} title="Nothing scheduled" hint="Add the first one with New schedule." />{/snippet}
    {#snippet children(_agents)}
      <div use:listStagger>
        <SettingsGroup>
          {#each withJobs as { agent, job } (`${agent.id}-${job.id}`)}
            <CronRow
              {job}
              agentLabel={agent.displayName}
              onAction={(a) => void act(agent.id, job.id, a)}
              onEdit={(patch) => edit(agent.id, job.id, patch)}
            />
          {/each}
        </SettingsGroup>
      </div>
    {/snippet}
  </QueryState>
  {#if unreachable.length > 0}
    <p transition:slide={{ duration: 150 }} class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
      <StatusDot status="warn" />
      Could not reach {unreachable.join(', ')}, so {unreachable.length === 1 ? 'its' : 'their'} schedules are not listed.
    </p>
  {/if}
  {#if summary && !composing}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-muted">{summary}</p>{/if}
</div>

{#if composing}
  <Modal open onClose={() => (composing = false)} title="New schedule" width="max-w-xl">
    <!-- While the roster is unknown — still loading OR failed — `chosen`
         would be an empty set and a schedule created now would target NOBODY.
         `isLoading` alone went false on a failure and re-armed the button. -->
    <CronForm bind:this={form} withActions={false} onCreate={create} {busy} disabled={query.data === undefined}>
      <SettingsField
        label="Agents"
        hint="Fixed times are staggered two minutes per agent."
        info="A schedule runs inside each agent and keeps firing even if Talaria is down. The stagger keeps them from all calling the models at once."
      >
        <div class="flex flex-wrap gap-1.5">
          {#if query.isLoading}
            {#each Array.from({ length: 4 }) as _, i (i)}
              <Skeleton class="h-[22px] w-20 rounded-md" />
            {/each}
          {/if}
          <!-- Target agents — the one filter-pill primitive (Chip). -->
          {#each agents as a (a.id)}
            <Chip onSelect={() => toggle(a.id)} selected={chosen.has(a.id)}>{a.displayName}</Chip>
          {/each}
        </div>
      </SettingsField>
    </CronForm>
    {#if summary}<p transition:slide={{ duration: 150 }} class="mt-3 font-sans text-[13px] leading-5 text-muted">{summary}</p>{/if}
    {#snippet footer()}
      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onclick={() => (composing = false)}>Cancel</Button>
        <Button size="sm" disabled={!form?.canSubmit()} onclick={() => form?.submit()}>{busy ? 'Creating' : 'Create schedule'}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}
