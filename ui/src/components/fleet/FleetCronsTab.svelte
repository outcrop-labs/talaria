<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { CalendarClock } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getList, postJson, putJson } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { listStagger, slide } from '@/lib/motion'
  import { type CronJob } from './agent-crons'
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

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['fleet-crons'],
    queryFn: (): Promise<FleetCronAgent[]> => getList<FleetCronAgent>('/api/fleet/crons', 'agents'),
  }))
  const agents = $derived(query.data ?? [])
  let selected = $state<Set<string> | null>(null) // null = all
  let busy = $state(false)
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
        summary = 'could not create jobs'
        return false
      }
      const failed = j.results.filter((x) => !x.ok)
      summary = failed.length === 0 ? `Created on ${j.results.length} agents (staggered).` : `Created on ${j.results.length - failed.length}, failed on ${failed.length}: ${failed[0]?.error ?? ''}`
      await qc.invalidateQueries({ queryKey: ['fleet-crons'] })
      return failed.length === 0
    } catch (e) {
      summary = errorMessage(e)
      return false
    } finally {
      busy = false
    }
  }

  const act = async (agentId: string, jobId: string, action: 'pause' | 'resume' | 'run' | 'remove') => {
    if (action === 'remove' && !(await confirm({ title: 'Delete scheduled job', message: 'Delete this scheduled job?', confirmLabel: 'Delete', danger: true }))) return
    try {
      if (action === 'remove') await delJson(`/api/fleet/agents/${agentId}/crons/${jobId}`)
      else await postJson(`/api/fleet/agents/${agentId}/crons/${jobId}`, { action })
    } catch (e) {
      // Row actions are fire-and-forget here (CronRow doesn't render an error
      // slot in the fleet tab) — a toast is where a failed one gets said.
      pushToast({ title: `${action.charAt(0).toUpperCase()}${action.slice(1)} failed`, body: errorMessage(e), tone: 'danger' })
    }
    await qc.invalidateQueries({ queryKey: ['fleet-crons'] })
  }

  const edit = async (agentId: string, jobId: string, patch: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    let saved = true
    try {
      await putJson(`/api/fleet/agents/${agentId}/crons/${jobId}`, patch)
    } catch (e) {
      saved = false
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
    }
    await qc.invalidateQueries({ queryKey: ['fleet-crons'] })
    return saved
  }

  const withJobs = $derived(agents.flatMap((a) => a.jobs.map((j) => ({ agent: a, job: j }))))
</script>

{#snippet title()}
  <span class="flex items-center gap-1.5">
    Schedules
    <InfoTip text="Jobs run inside each agent's own scheduler and keep firing even if Talaria is down. Fixed-time jobs created here are staggered 2 minutes per agent so they don't all hit the models at once." />
  </span>
{/snippet}

{#snippet calendarIcon()}<CalendarClock size={22} />{/snippet}

<div class="space-y-5">
  <div class="flex items-center gap-2">
    {@render title()}
  </div>
    <QueryState {query} errorTitle="Could not load fleet schedules" errorVariant="compact" isEmpty={() => withJobs.length === 0}>
      {#snippet skeleton()}<CronListSkeleton />{/snippet}
      {#snippet empty()}<EmptyState icon={calendarIcon} title="Nothing scheduled anywhere" hint="Create the first job below." />{/snippet}
      {#snippet children(_agents)}
        <!-- div, not ul: CronRow renders div rows. -->
        <div class="divide-y divide-line rounded-lg border border-line" use:listStagger>
          {#each withJobs as { agent, job } (`${agent.id}-${job.id}`)}
            <CronRow
              {job}
              agentLabel={agent.displayName}
              onAction={(a) => void act(agent.id, job.id, a)}
              onEdit={(patch) => edit(agent.id, job.id, patch)}
            />
          {/each}
        </div>
      {/snippet}
    </QueryState>
    {#if agents.some((a) => a.error)}
      <p transition:slide={{ duration: 150 }} class="text-xs text-warning">
        Unreachable: {agents.filter((a) => a.error).map((a) => a.displayName).join(', ')}. Are they running?
      </p>
    {/if}

    <div>
      <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">New job across agents</div>
      <!-- While the roster is unknown — still loading OR failed — `chosen`
           would be an empty set and a job created now would target NOBODY.
           `isLoading` alone went false on a failure and re-armed the button. -->
      <CronForm onCreate={create} {busy} disabled={query.data === undefined}>
        <div class="flex flex-wrap gap-1.5">
          {#if query.isLoading}
            {#each Array.from({ length: 4 }) as _, i (i)}
              <Skeleton class="h-6 w-20 rounded" />
            {/each}
          {/if}
          <!-- Target agents — the one filter-pill primitive (Chip). -->
          {#each agents as a (a.id)}
            <Chip onSelect={() => toggle(a.id)} selected={chosen.has(a.id)} class="px-2.5 py-0.5">
              {a.displayName}
            </Chip>
          {/each}
        </div>
      </CronForm>
      {#if summary}<p transition:slide={{ duration: 150 }} class="mt-2 text-xs text-muted">{summary}</p>{/if}
    </div>
</div>
