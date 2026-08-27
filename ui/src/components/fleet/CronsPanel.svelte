<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { CalendarClock } from '@lucide/svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getList, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { type CronJob } from './agent-crons'
  import CronForm from './CronForm.svelte'
  import CronRow from './CronRow.svelte'

  // One agent's cron jobs: list + create + edit/pause/run/delete.
  let { agentId }: { agentId: string } = $props()

  const qc = useQueryClient()
  const key = () => ['agent-crons', agentId]
  // Already rejected on non-2xx and already had an error branch; routed through
  // `getList` so the "200 but no `jobs` wrapper" case reads the same as it does
  // everywhere else.
  const query = createQuery(() => ({
    queryKey: key(),
    queryFn: (): Promise<CronJob[]> => getList<CronJob>(`/api/fleet/agents/${agentId}/crons`, 'jobs'),
  }))
  let busy = $state(false)
  let err = $state<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: key() })

  const create = async (input: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    busy = true
    err = null
    try {
      await postJson(`/api/fleet/agents/${agentId}/crons`, input)
      await refresh()
      return true
    } catch (e) {
      err = errorMessage(e)
      return false
    } finally {
      busy = false
    }
  }

  const edit = async (jobId: string, patch: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    busy = true
    err = null
    try {
      await putJson(`/api/fleet/agents/${agentId}/crons/${jobId}`, patch)
      await refresh()
      return true
    } catch (e) {
      err = errorMessage(e)
      return false
    } finally {
      busy = false
    }
  }

  const act = async (jobId: string, action: 'pause' | 'resume' | 'run' | 'remove') => {
    if (action === 'remove' && !(await confirm({ title: 'Delete scheduled job', message: 'Delete this scheduled job?', confirmLabel: 'Delete', danger: true }))) return
    busy = true
    err = null
    try {
      if (action === 'remove') await delJson(`/api/fleet/agents/${agentId}/crons/${jobId}`)
      else await postJson(`/api/fleet/agents/${agentId}/crons/${jobId}`, { action })
      await refresh()
    } catch (e) {
      err = errorMessage(e)
      // The list is re-read on failure too, as it always was — a rejected
      // action may still have partly applied server-side.
      await refresh()
    } finally {
      busy = false
    }
  }
</script>

{#snippet calendarIcon()}<CalendarClock size={22} />{/snippet}

<!-- One job row's silhouette (see CronRow: dot + name + schedule, next-run
     meta, then the three action tiles) — same px-3.5 py-3 frame. -->
{#snippet cronRowSkeleton(i: number)}
  <div aria-hidden="true" class="flex items-center gap-2.5 px-3.5 py-3">
    <!-- Fixed geometry, so nothing about it is uncertain — a flat rail, not a skeleton (UI-CONVENTIONS, Loading). -->
    <div class="h-2 w-2 shrink-0 rounded-full bg-line"></div>
    <Skeleton class={`h-3 rounded-full ${['w-36', 'w-28', 'w-44'][i % 3]}`} />
    <Skeleton class={`h-2.5 rounded-full ${['w-28', 'w-20', 'w-24'][i % 3]}`} />
    <span class="ml-auto flex shrink-0 items-center gap-1">
      <Skeleton class="h-7 w-7 rounded-md" />
      <Skeleton class="h-7 w-7 rounded-md" />
      <Skeleton class="h-7 w-7 rounded-md" />
    </span>
  </div>
{/snippet}

<div class="space-y-4">
  <div class="flex items-center gap-1.5">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Schedules</span>
    <InfoTip text="Recurring jobs the agent runs on its own native scheduler; they keep firing even when Talaria is down." />
  </div>
  <!-- Skeleton → content as one motion: row-shaped skeletons materialize into
       the job rows. QueryState keeps owning ERROR and EMPTY; the bordered
       frame only applies while there are rows (or their skeletons) to hold,
       so the empty/error states render unframed as before. -->
  <Materialize
    loading={query.isLoading}
    count={3}
    class={query.isLoading || (query.data?.length ?? 0) > 0 ? 'divide-y divide-line rounded-lg border border-line' : undefined}
  >
    {#snippet skeleton(i)}{@render cronRowSkeleton(i)}{/snippet}
    <QueryState {query} errorTitle="Schedules unavailable" errorVariant="compact">
      {#snippet skeleton()}{@render cronRowSkeleton(0)}{/snippet}
      {#snippet empty()}<EmptyState icon={calendarIcon} title="Nothing scheduled" hint="Give it a recurring job below." />{/snippet}
      {#snippet children(jobs)}
        {#each jobs as j (j.id)}
          <CronRow job={j} {busy} onAction={(a) => void act(j.id, a)} onEdit={(patch) => edit(j.id, patch)} />
        {/each}
      {/snippet}
    </QueryState>
  </Materialize>
  <CronForm onCreate={create} {busy} />
  {#if err}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{err}</p>{/if}
</div>
