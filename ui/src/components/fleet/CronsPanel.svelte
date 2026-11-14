<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { CalendarClock } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { getList } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { type CronFormHandle, type CronJob, type CronMutationState, useCronMutations } from './agent-crons'
  import CronForm from './CronForm.svelte'
  import CronListSkeleton from './CronListSkeleton.svelte'
  import CronRow from './CronRow.svelte'
  import CronTable from './CronTable.svelte'

  // One agent's cron jobs: list + create + edit/pause/run/delete. Reading is
  // open to anyone who can see the agent; every write belongs to its managers
  // (docs/PERMISSIONS.md, "Agent managers"), so a non-manager gets the rows
  // without the controls rather than a row of buttons that 403.
  let { agentId, canManage }: { agentId: string; canManage: boolean } = $props()

  const qc = useQueryClient()
  const key = () => ['agent-crons', agentId]
  // Already rejected on non-2xx and already had an error branch; routed through
  // `getList` so the "200 but no `jobs` wrapper" case reads the same as it does
  // everywhere else.
  const query = createQuery(() => ({
    queryKey: key(),
    queryFn: (): Promise<CronJob[]> => getList<CronJob>(`/api/fleet/agents/${agentId}/crons`, 'jobs'),
  }))

  let composing = $state(false)
  let form = $state<CronFormHandle | null>(null)

  // What this panel renders about a mutation: the error inline under the rows,
  // and `busy` on every control. The hook drives both.
  const ui = $state<CronMutationState>({ err: null, busy: false })
  const { create, edit, act } = useCronMutations({
    state: ui,
    invalidate: (what, failed) => {
      // A rejected row action re-reads the list — it may still have partly
      // applied server-side. A rejected create/edit changed nothing, and was
      // never re-read here.
      if (failed && (what === 'create' || what === 'edit')) return
      return qc.invalidateQueries({ queryKey: key() })
    },
  })
</script>

{#snippet calendarIcon()}<CalendarClock size={22} />{/snippet}

<div class="space-y-4">
  <div class="flex items-center gap-4">
    <p class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">
      Recurring work this agent starts by itself. It keeps running even when Talaria is down.
    </p>
    {#if canManage}
      <Button variant="outline" size="sm" class="shrink-0" onclick={() => (composing = true)}>
        New schedule
      </Button>
    {/if}
  </div>
  <QueryState {query} errorTitle="Schedules unavailable" errorVariant="compact">
    {#snippet skeleton()}<CronListSkeleton />{/snippet}
    {#snippet empty()}
      <EmptyState
        icon={calendarIcon}
        title="Nothing scheduled"
        hint={canManage ? 'Give it recurring work with New schedule.' : "Only this agent's managers can schedule its work."}
      />
    {/snippet}
    {#snippet children(jobs)}
      <CronTable>
        {#each jobs as j (j.id)}
          <CronRow job={j} busy={ui.busy || !canManage} onAction={(a) => void act(agentId, j.id, a)} onEdit={(patch) => edit(agentId, j.id, patch)} />
        {/each}
      </CronTable>
    {/snippet}
  </QueryState>
  {#if ui.err && !composing}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{ui.err}</p>{/if}
</div>

{#snippet composeFooter()}
  <div class="flex justify-end gap-2">
    <Button variant="ghost" size="sm" onclick={() => (composing = false)}>Cancel</Button>
    <Button size="sm" disabled={!form?.canSubmit()} onclick={() => form?.submit()}>{ui.busy ? 'Creating' : 'Create schedule'}</Button>
  </div>
{/snippet}

{#if composing}
  <Modal open onClose={() => (composing = false)} title="New schedule" width="max-w-xl" footer={composeFooter}>
    <CronForm
      bind:this={form}
      withActions={false}
      onCreate={async (input) => {
        const created = await create(agentId, input)
        if (created) composing = false
        return created
      }}
      busy={ui.busy}
    />
    {#if ui.err}<p transition:slide={{ duration: 150 }} class="mt-3 font-sans text-[13px] leading-5 text-danger" role="alert">{ui.err}</p>{/if}
  </Modal>
{/if}
