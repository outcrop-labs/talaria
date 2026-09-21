<script lang="ts">
  // One workchain as a horizontal rail: a header (name, progress, the chain's
  // own controls) and its steps as compact cards joined by chevrons. The step
  // cards are SUMMARIES off the chain read, not full Task rows — so this file
  // renders Avatar/StatusDot/due text directly rather than through the pills,
  // which want a Task and a mutation context. Derived states wear the styling:
  // done fades with a check, head carries the accent ring, waiting stays
  // quiet, archived strikes through. Clicking a card opens the ticket.
  import { Check, ChevronRight, Pause, Play, Plus } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { assigneeInfo, type AssigneeInfo } from '@/lib/assignees'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { EFFORT_LABEL, type Task } from '@/lib/task-const'
  import { statusColorOf, type BoardStatus } from '@/lib/statuses'
  import { isOverdueTask } from '@/components/board/field-pills'
  import { addWorkchainStep, updateWorkchain } from '@/lib/workchain-client'
  import { chainProgress, type Workchain } from '@/lib/workchain-rules'
  import type { AgentModel } from '@/lib/agents'
  import type { BoardMember } from '@/lib/boards.svelte'

  let {
    workchain,
    boardStatuses,
    agents,
    members,
    pickable,
    onOpen,
    onAdded,
  }: {
    workchain: Workchain
    boardStatuses: BoardStatus[]
    agents: Array<Pick<AgentModel, 'id' | 'label'>>
    members: BoardMember[]
    /** Tickets that may join a chain: unarchived, not in any chain. */
    pickable: Task[]
    onOpen: (taskId: string) => void
    /** The rail mutated; the owner re-reads (it owns the query). */
    onAdded: () => void
  } = $props()

  const infos = (step: (typeof workchain.steps)[number]): AssigneeInfo[] =>
    step.assignees.map((a) => assigneeInfo(a, agents, members))

  const fmtDue = (iso: string) => {
    const d = new Date(iso)
    const sameYear = d.getFullYear() === new Date().getFullYear()
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) })
  }

  const failure = (what: string) => (e: unknown) =>
    pushToast({ title: `${what} failed`, body: errorMessage(e), tone: 'danger' })

  const togglePaused = () =>
    void updateWorkchain(workchain.id, { paused: !workchain.paused })
      .then(onAdded)
      .catch(failure(workchain.paused ? 'Unpausing the chain' : 'Pausing the chain'))

  const addStep = (taskId: string, close: () => void) => {
    close()
    void addWorkchainStep(workchain.id, taskId).then(onAdded).catch(failure('Adding the step'))
  }

  let search = $state('')

  const pickOptions = $derived(
    pickable
      .map((t) => ({ t, hay: `${t.title} ${t.ticketRef ?? ''} ${t.tags.join(' ')}`.toLowerCase() }))
      .filter(({ hay }) => hay.includes(search.trim().toLowerCase()))
      .slice(0, 30),
  )
</script>

<div class="min-w-0">
  <!-- Rail header: the chain's name, its derived progress, and its controls.
       Paused is a quiet mono marker, not a modal alert — the ring of the
       head card is what says where the chain is. -->
  <div class="flex flex-wrap items-center gap-2">
    <span class="font-sans text-sm font-medium text-fg">{workchain.name}</span>
    <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{chainProgress(workchain)}</span>
    {#if workchain.paused}
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-warning">paused</span>
    {/if}
    {#if workchain.createdBy}
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{workchain.createdBy}</span>
    {/if}
    <span class="flex items-center gap-1">
      <IconButton
        size="sm"
        title={workchain.paused ? 'Unpause chain' : 'Pause chain'}
        onclick={togglePaused}
      >
        {#if workchain.paused}<Play size={14} />{:else}<Pause size={14} />{/if}
      </IconButton>
      {#if pickable.length > 0}
        <Popover align="left">
          {#snippet trigger(open)}
            <IconButton size="sm" active={open} title="Add a ticket to this chain">
              <Plus size={14} />
            </IconButton>
          {/snippet}
          {#snippet content(close)}
            <div class="w-64 space-y-2">
              <Input
                value={search}
                oninput={(e) => (search = e.currentTarget.value)}
                placeholder="Search tickets"
                size="sm"
              />
              <div class="max-h-64 overflow-y-auto">
                {#if pickOptions.length === 0}
                  <div class="px-2 py-3 text-center font-sans text-xs text-muted">No tickets match.</div>
                {:else}
                  {#each pickOptions as { t } (t.id)}
                    <button
                      type="button"
                      class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-raised"
                      onclick={() => addStep(t.id, close)}
                    >
                      <StatusDot color={statusColorOf(t.status, boardStatuses)} class="mt-0.5" />
                      <span class="min-w-0 flex-1">
                        <span class="block truncate font-sans text-xs text-fg">{t.title}</span>
                      </span>
                      {#if t.ticketRef}
                        <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.ticketRef}</span>
                      {/if}
                    </button>
                  {/each}
                {/if}
              </div>
            </div>
          {/snippet}
        </Popover>
      {/if}
    </span>
  </div>

  <!-- The rail: compact cards on a horizontal scroll, chevrons joining them.
       A 0-step chain still renders — the add picker is the only way to seed
       it, and an empty rail with a picker beats a chain that cannot exist. -->
  <div class="mt-2 flex items-stretch gap-1.5 overflow-x-auto pb-1">
    {#if workchain.steps.length === 0}
      <div
        class="flex w-52 shrink-0 items-center rounded-lg border border-dashed border-line-subtle px-3 py-4 font-sans text-xs text-muted"
      >
        {#if pickable.length > 0}
          Empty chain — add its first ticket.
        {:else}
          Empty chain — every ticket is already chained or archived.
        {/if}
      </div>
    {:else}
      {#each workchain.steps as step, i (step.taskId)}
        {#if i > 0}
          <ChevronRight size={14} class="shrink-0 self-center text-line-strong" />
        {/if}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          role="button"
          tabindex={0}
          onclick={() => onOpen(step.taskId)}
          onkeydown={(e) => {
            if (e.key === 'Enter' && e.target === e.currentTarget) onOpen(step.taskId)
          }}
          class={cn(
            'w-52 shrink-0 cursor-pointer rounded-lg border border-line bg-panel p-3 text-left transition-colors hover:border-line-strong',
            step.state === 'head' && 'ring-1 ring-inset ring-accent',
            (step.state === 'done' || step.state === 'archived') && 'opacity-50',
          )}
        >
          <div class="flex items-center gap-1.5">
            <StatusDot color={statusColorOf(step.status, boardStatuses)} />
            {#if step.ticketRef}
              <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{step.ticketRef}</span>
            {/if}
            {#if step.state === 'done'}
              <Check size={12} class="ml-auto shrink-0 text-success" />
            {:else if step.state === 'head'}
              <span class="ml-auto font-mono text-[9px] uppercase tracking-[0.05em] text-accent">head</span>
            {/if}
          </div>
          <div class={cn('mt-1 font-sans text-[13px] font-medium leading-snug text-fg', step.state === 'archived' && 'line-through')}>
            {step.title}
          </div>
          <div class="mt-2 flex items-center gap-2">
            {#if step.effort}
              <span class="rounded border border-line-subtle px-1 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">
                {EFFORT_LABEL[step.effort]}
              </span>
            {/if}
            {#if step.dueDate}
              {@const overdue = isOverdueTask({ dueDate: step.dueDate, status: step.status }, boardStatuses)}
              <span class={cn('font-mono text-[10px] tracking-[0.05em]', overdue ? 'font-medium text-danger' : 'text-muted')}>
                {fmtDue(step.dueDate)}
              </span>
            {/if}
            {#if step.assignees.length > 0}
              <span class="ml-auto flex -space-x-1.5">
                {#each infos(step).slice(0, 3) as a (a.key)}
                  <Avatar name={a.label} class="h-4.5 w-4.5 ring-2 ring-[color:var(--theme-panel)]" />
                {/each}
              </span>
            {/if}
          </div>
        </div>
      {/each}
    {/if}
  </div>
</div>
