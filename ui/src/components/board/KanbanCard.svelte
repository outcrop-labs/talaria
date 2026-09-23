<script lang="ts">
  import { Eye, MessageSquare, GitBranch } from '@lucide/svelte'
  import CopyLinkButton from '@/components/ui/CopyLinkButton.svelte'
  import DitherBorder from '@/components/ui/DitherBorder.svelte'
  import DitherLayer from '@/components/ui/DitherLayer.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import AssigneesPill from './AssigneesPill.svelte'
  import DuePill from './DuePill.svelte'
  import EstimatePill from './EstimatePill.svelte'
  import LabelsPill from './LabelsPill.svelte'
  import { LABEL_CSS, isClosedStatus, type PillCtx } from './field-pills'
  import { fmtHours } from './kanban'
  import { cn } from '@/lib/cn'
  import { plainText } from '@/lib/plain-text'
  import { EFFORT_LABEL, PRIORITY_COLOR, pgNum, type Task } from '@/lib/task-const'

  let {
    task,
    pillCtx,
    subtasks,
    parentRef,
    draggable,
    dim,
    onDragStart,
    onDragEnd,
    onOpen,
    onContextMenu,
    onWatch,
    session = null,
    wait = null,
  }: {
    task: Task
    session?: { runId: string; agentModel: string | null; turn: number | null } | null
    wait?: { reason: string; phase: string } | null
    onWatch?: () => void
    pillCtx: PillCtx
    subtasks: Task[]
    parentRef: string | null
    draggable: boolean
    dim: boolean
    onDragStart: (e: DragEvent) => void
    onDragEnd: () => void
    onOpen: () => void
    onContextMenu: (e: MouseEvent) => void
  } = $props()

  // `isClosedStatus`, not `s.status === 'done'`. The literal asked the DEFAULT
  // workflow's question: on a board whose done column is `shipped` or `merged`
  // — which custom statuses make legal — every finished sub-task counted as
  // outstanding and the rollup read "0/5" forever. It also disagreed with the
  // due pill on the same card, which has always asked `isClosedStatus` through
  // `isOverdueTask`: a sub-task in a custom done column was simultaneously "not
  // done" here and "not overdue" there.
  const doneKids = $derived(subtasks.filter((s) => isClosedStatus(s.status, pillCtx.statuses)).length)
  // Wire numeric → number once, at the top, rather than at each read.
  const estimate = $derived(pgNum(task.estimatedHours))
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  {draggable}
  ondragstart={onDragStart}
  ondragend={onDragEnd}
  oncontextmenu={onContextMenu}
  class={cn('group relative cursor-grab active:cursor-grabbing', dim && 'opacity-40')}
>
  <!-- Hover corners: the copy affordance and — while work is live or queued
       on this ticket — the watch EYE directly beside it, same reveal, same
       grammar. The eye replaces the old sliding mono 'watch' chip: the copy
       button no longer needs to move, and a queued ticket shows the eye
       disabled (no run to open until packing frees a slot). -->
  <div class="absolute right-2 top-2 z-10 flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
    {#if session || wait}
      <button
        type="button"
        title="Watch the work"
        aria-label="Watch the work"
        disabled={!session}
        onclick={(e) => {
          e.stopPropagation()
          onWatch?.()
        }}
        class="flex items-center rounded-md bg-raised p-1 text-accent shadow-[var(--theme-shadow-1)] transition-colors hover:text-fg disabled:opacity-50"
      >
        <Eye size={13} />
      </button>
    {/if}
    <CopyLinkButton
      path={`/boards/${task.boardId}/${task.id}`}
      class="bg-raised shadow-[var(--theme-shadow-1)]"
    />
  </div>
  <!-- div, not <button>: the pills inside are buttons themselves. -->
  <div
    role="button"
    tabindex={0}
    onclick={onOpen}
    onkeydown={(e) => {
      if (e.key === 'Enter' && e.target === e.currentTarget) onOpen()
    }}
    class={cn(
      // §8 panel: panel surface + hairline + radius 8; hover raises the
      // hairline (matte — no glow, ~120ms).
      'relative w-full cursor-pointer overflow-hidden rounded-lg border border-line bg-panel p-4 text-left shadow-[var(--theme-shadow-1)] transition-colors hover:border-line-strong',
      task.archivedAt && 'opacity-60',
    )}
  >
  {#if session || wait}
    <div class="pointer-events-none absolute inset-0 z-[5]" aria-hidden="true">
      <DitherLayer
        sources={[
          { id: 'lull', kind: 'edge', side: 'top', depth: 34, strength: 0.16 },
          { id: 'drift', kind: 'wave', axis: 'x', wavelength: 200, speed: 12, strength: 0.3 },
        ]}
        pitch={4}
        dot={1.4}
        alphaFloor={0.04}
        maxAlpha={0.26}
      />
    </div>
    <!-- The trace: while a session is LIVE, the same field the wash renders
         is masked to a hairline ring around the panel — the edge that reads
         even where the wash is too faint. Additive with the overlay above;
         the watch eye in the corner is the affordance now. -->
    {#if session}
      <DitherBorder
        sources={[
          { id: 'trace', kind: 'edge', side: 'top', depth: 30, strength: 0.22 },
          { id: 'trace-drift', kind: 'wave', axis: 'x', wavelength: 200, speed: 12, strength: 0.5 },
        ]}
        pitch={4}
        dot={1.4}
        alphaFloor={0.06}
        maxAlpha={0.5}
      />
    {/if}
    {#if wait}
      <span
        class="absolute right-14 top-2 z-10 max-w-[10rem] truncate rounded-md border border-line bg-raised/80 px-1.5 py-0.5 font-mono text-[10px] text-muted"
        title="{wait.phase} — {wait.reason}"
      >
        queued
      </span>
    {/if}
  {/if}
    <!-- Color-code stripe (ticket color, when set). -->
    {#if task.color}
      <span class="absolute inset-y-0 left-0 w-1" style:background={LABEL_CSS[task.color]}></span>
    {/if}
    <div class="flex items-start gap-2.5">
      <StatusDot color={PRIORITY_COLOR[task.priority]} class="mt-2" />
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-1.5">
          {#if task.ticketRef}
            <span class="font-mono text-[11px] tracking-[0.05em] text-muted">{task.ticketRef}</span>
          {/if}
          {#if parentRef}
            <span class="truncate font-mono text-[10px] text-muted" title={`Sub-task of ${parentRef}`}>
              ↳ {parentRef}
            </span>
          {/if}
          {#if task.effort}
            <span class="rounded border border-line-subtle px-1 font-mono text-[9px] font-medium uppercase tracking-[0.05em] text-muted">{EFFORT_LABEL[task.effort]}</span>
          {/if}
          {#if estimate != null}
            <span class="rounded border border-line-subtle px-1 font-mono text-[9px] font-medium tracking-[0.05em] text-muted" title="Estimate">
              {fmtHours(estimate)}
            </span>
          {/if}
          {#if task.archivedAt}
            <span class="rounded border border-line-subtle px-1 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">archived</span>
          {/if}
        </div>
        <div class="font-sans text-[15px] font-medium leading-snug text-fg">{task.title}</div>
        {#if task.description}
          <div class="mt-1 line-clamp-3 font-sans text-xs leading-relaxed text-muted">{plainText(task.description)}</div>
        {/if}
      </div>
    </div>
    <!-- Property pills — PERSISTENT controls: bordered, labeled, chevroned.
        What you can change is never a mystery. -->
    <div class="mt-2.5 flex flex-wrap items-center gap-1">
      <AssigneesPill t={task} ctx={pillCtx} persistent />
      <DuePill t={task} ctx={pillCtx} persistent />
      <EstimatePill t={task} ctx={pillCtx} persistent />
      <LabelsPill t={task} ctx={pillCtx} persistent />
      <span class="ml-auto flex shrink-0 items-center gap-1.5">
        {#if subtasks.length > 0}
          <span
            class={cn('flex items-center gap-1 font-mono text-[11px] tracking-[0.05em]', doneKids === subtasks.length ? 'text-success' : 'text-muted')}
            title="Sub-tasks done"
          >
            <GitBranch size={11} /> {doneKids}/{subtasks.length}
          </span>
        {/if}
        {#if task.commentCount > 0}
          <span class="flex items-center gap-1 font-mono text-[11px] tracking-[0.05em] text-muted" title="Comments">
            <MessageSquare size={11} /> {task.commentCount}
          </span>
        {/if}
      </span>
    </div>
  </div>
</div>
