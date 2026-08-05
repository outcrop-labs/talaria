<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { CalendarClock } from '@lucide/svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { getList } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { type CronJob } from './agent-crons'
  import CronForm from './CronForm.svelte'
  import CronListSkeleton from './CronListSkeleton.svelte'
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
      const r = await fetch(`/api/fleet/agents/${agentId}/crons`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(input),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) {
        err = j.error ?? 'could not create the job'
        return false
      }
      await refresh()
      return true
    } finally {
      busy = false
    }
  }

  const edit = async (jobId: string, patch: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    busy = true
    err = null
    try {
      const r = await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(patch),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) {
        err = j.error ?? 'could not save the job'
        return false
      }
      await refresh()
      return true
    } finally {
      busy = false
    }
  }

  const act = async (jobId: string, action: 'pause' | 'resume' | 'run' | 'remove') => {
    if (action === 'remove' && !(await confirm({ title: 'Delete scheduled job', message: 'Delete this scheduled job?', confirmLabel: 'Delete', danger: true }))) return
    busy = true
    err = null
    try {
      const r =
        action === 'remove'
          ? await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, { method: 'DELETE', credentials: 'same-origin' })
          : await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, {
              method: 'POST',
              credentials: 'same-origin',
              headers: { 'content-type': 'application/json' },
              body: JSON.stringify({ action }),
            })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) err = j.error ?? `could not ${action}`
      await refresh()
    } finally {
      busy = false
    }
  }
</script>

{#snippet calendarIcon()}<CalendarClock size={22} />{/snippet}

<div class="space-y-4">
  <div class="flex items-center gap-1.5">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Schedules</span>
    <InfoTip text="Recurring jobs the agent runs on its own native scheduler — they keep firing even when Talaria is down." />
  </div>
  <QueryState {query} errorTitle="Schedules unavailable" errorVariant="compact">
    {#snippet skeleton()}<CronListSkeleton />{/snippet}
    {#snippet empty()}<EmptyState icon={calendarIcon} title="Nothing scheduled" hint="Give it a recurring job below." />{/snippet}
    {#snippet children(jobs)}
      <ul class="divide-y divide-line rounded-lg border border-line">
        {#each jobs as j (j.id)}
          <CronRow job={j} {busy} onAction={(a) => void act(j.id, a)} onEdit={(patch) => edit(j.id, patch)} />
        {/each}
      </ul>
    {/snippet}
  </QueryState>
  <CronForm onCreate={create} {busy} />
  {#if err}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{err}</p>{/if}
</div>
