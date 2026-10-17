<script lang="ts">
  // WHAT THIS SESSION IS ALREADY SCHEDULED TO DO. One row per standing job on
  // the thread's agent whose runs post back INTO this thread, read from
  // `/api/conversations/{id}/schedules` — the stored row plus the agent's own
  // live view of the job when its container could be asked.
  //
  // READ-ONLY EXCEPT FOR REMOVE, deliberately. Pausing, editing and run-now
  // belong to the agent's own schedules panel (components/fleet/CronsPanel),
  // which has the room for them; what a session needs at hand is what it is
  // on the hook for, and the way to stop one. Remove is the quiet DangerLink
  // the house gives destructive actions, with the confirm as the deliberate
  // step.
  import { useQueryClient } from '@tanstack/svelte-query'
  import { CalendarClock } from '@lucide/svelte'
  import { describeSchedule } from '@/components/fleet/agent-crons'
  import { confirm } from '@/components/ui/confirm.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { scheduleTone, unscheduleFromConversation, useConversationSchedules, type ScheduleRow } from '@/lib/schedules'
  import { toastError } from '@/lib/toast.svelte'
  import { TONE_DOT, TONE_TITLE, nextRunLabel } from './schedules-panel'

  let { conversationId }: { conversationId: string } = $props()

  const qc = useQueryClient()
  const query = useConversationSchedules(() => conversationId)
  // Which row is going, so its own controls hold rather than the whole list.
  let removing = $state<string | null>(null)

  const remove = async (row: ScheduleRow) => {
    if (
      !(await confirm({
        title: 'Remove schedule',
        message: `Stop "${row.name}" from running? The turns it already posted stay in this session.`,
        confirmLabel: 'Remove',
        danger: true,
      }))
    )
      return
    removing = row.jobId
    try {
      await unscheduleFromConversation(conversationId, row.jobId)
    } catch (e) {
      // The panel has no error line of its own: a refused remove is news
      // about the request, and the toast is where this surface says it.
      toastError('Could not remove the schedule', e)
    } finally {
      removing = null
      void qc.invalidateQueries({ queryKey: ['conversation-schedules', conversationId] })
    }
  }
</script>

{#snippet calendarIcon()}<CalendarClock size={18} />{/snippet}

<!-- One row's silhouette: the leading dot is FIXED geometry whatever the data
     turns out to be, so it is a flat rail rather than static (UI-CONVENTIONS,
     Loading). -->
{#snippet rowSkeleton(i: number)}
  <div aria-hidden="true" class="flex items-center gap-2.5 px-3 py-2.5">
    <div class="h-[7px] w-[7px] shrink-0 rounded-full bg-line"></div>
    <Skeleton class={`h-3 rounded-full ${['w-32', 'w-24'][i % 2]}`} />
    <Skeleton class={`h-2.5 rounded-full ${['w-28', 'w-36'][i % 2]}`} />
    <Skeleton class="ml-auto h-2.5 w-20 rounded-full" />
  </div>
{/snippet}

<div>
  <SectionHeader title="On this session" info="Standing jobs on this agent whose every run posts its answer back into this session." />
  <QueryState {query} errorTitle="Schedules unavailable" errorVariant="compact">
    {#snippet skeleton()}
      <div class="divide-y divide-line rounded-lg border border-line">
        {@render rowSkeleton(0)}
        {@render rowSkeleton(1)}
      </div>
    {/snippet}
    {#snippet empty()}
      <EmptyState
        variant="compact"
        icon={calendarIcon}
        title="Nothing scheduled"
        hint="A job created above shows up here, with its next run."
      />
    {/snippet}
    {#snippet children(rows)}
      <div class="divide-y divide-line rounded-lg border border-line">
        {#each rows as row (row.jobId)}
          {@const tone = scheduleTone(row)}
          {@const next = nextRunLabel(row)}
          <div class="flex items-center gap-2.5 px-3 py-2.5">
            <StatusDot status={TONE_DOT[tone]} title={TONE_TITLE[tone]} class="h-[7px] w-[7px]" />
            <span class="min-w-0 flex-1">
              <span class="font-sans text-sm font-medium text-fg">{row.name}</span>
              <!-- Plain English, with the cron string in the tooltip — the
                   schedule reads the same way everywhere in the app. -->
              <span class="ml-2 font-sans text-xs text-accent" title={row.schedule}>
                {describeSchedule(row.schedule)}
              </span>
            </span>
            {#if next}<span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{next}</span>{/if}
            <DangerLink class="shrink-0" disabled={removing === row.jobId} onClick={() => void remove(row)}>
              Remove
            </DangerLink>
          </div>
        {/each}
      </div>
    {/snippet}
  </QueryState>
</div>
