<script lang="ts">
  // One workchain as a NODE CANVAS (TALA-35): free-placed step cards with a
  // visible in-port (left) and out-port (right), wires as cubic beziers, and
  // wire state mirroring the derived step state — idle gray, fired accent
  // (the handoff rode it), done success. Cards drag; positions persist
  // through PATCH nodes. Unplaced steps auto-layout by longest-path levels.
  //
  // The canvas draws the graph; the drag-TO-CONNECT editor (pulling a wire
  // out of a port) is TALA-34 and lands on this model. Wire deletion here is
  // a click on a wire (editors only) — the same write the edge DELETE serves.
  import { Check, Pause, Play } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { assigneeInfo } from '@/lib/assignees'
  import { cn } from '@/lib/cn'
  import { formatShortDate } from '@/lib/format'
  import { toastError } from '@/lib/toast.svelte'
  import { EFFORT_LABEL } from '@/lib/task-const'
  import { statusColorOf, type BoardStatus } from '@/lib/statuses'
  import { isOverdueTask } from '@/components/board/field-pills'
  import { updateWorkchain } from '@/lib/workchain-client'
  import {
    autoLayout,
    chainProgress,
    NODE_H,
    NODE_W,
    wirePath,
    wireState,
    type Workchain,
    type WorkchainStep,
  } from '@/lib/workchain-rules'
  import type { AgentModel } from '@/lib/agents'
  import type { BoardMember } from '@/lib/boards.svelte'

  let {
    workchain,
    boardStatuses,
    agents,
    members,
    onOpen,
    onChanged,
  }: {
    workchain: Workchain
    boardStatuses: BoardStatus[]
    agents: Array<Pick<AgentModel, 'id' | 'label'>>
    members: BoardMember[]
    onOpen: (taskId: string) => void
    /** A write landed; the owner re-reads (it owns the query). */
    onChanged: () => void
  } = $props()

  const info = (step: WorkchainStep) =>
    step.assignees.map((a) => assigneeInfo(a, agents, members))

  const failure = (what: string) => (e: unknown) =>
    toastError(`${what} failed`, e)

  const togglePaused = () =>
    void updateWorkchain(workchain.id, { paused: !workchain.paused })
      .then(onChanged)
      .catch(failure(workchain.paused ? 'Unpausing the chain' : 'Pausing the chain'))

  // ── The graph layout ─────────────────────────────────────────────────────
  const byTask = $derived(new Map(workchain.steps.map((s) => [s.taskId, s])))
  const positions = $derived(autoLayout(workchain.steps, workchain.edges))

  // Drag: a moved card writes its new spot (per axis, coalesced server-side).
  // The drag is local-first: positions live in a local overlay so the card
  // follows the cursor 1:1, and the PATCH fires on release.
  let dragId = $state<string | null>(null)
  let dragPos = $state<{ x: number; y: number } | null>(null)
  let dragStart = $state<{ mx: number; my: number; ox: number; oy: number } | null>(null)

  const nodePos = (taskId: string): { x: number; y: number } =>
    dragId === taskId && dragPos ? dragPos : (positions.get(taskId) ?? { x: 0, y: 0 })

  const startDrag = (taskId: string, e: PointerEvent) => {
    const p = positions.get(taskId) ?? { x: 0, y: 0 }
    dragId = taskId
    dragPos = { ...p }
    dragStart = { mx: e.clientX, my: e.clientY, ox: p.x, oy: p.y }
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  }

  const moveDrag = (e: PointerEvent) => {
    if (!dragId || !dragStart) return
    dragPos = { x: dragStart.ox + (e.clientX - dragStart.mx), y: dragStart.oy + (e.clientY - dragStart.my) }
  }

  const endDrag = () => {
    if (!dragId || !dragPos) return
    const { x, y } = dragPos
    const id = dragId
    dragId = null
    dragPos = null
    dragStart = null
    // Round to the grid the card snapped out of; a click (no move) skips
    // the write — the open-ticket handler owns clicks.
    void updateWorkchain(workchain.id, { nodes: [{ taskId: id, x: Math.round(x), y: Math.round(y) }] })
      .then(onChanged)
      .catch(failure('Moving the card'))
  }

  const canvasSize = $derived({
    w: Math.max(0, ...[...positions.values()].map((p) => p.x + NODE_W)) + 80,
    h: Math.max(0, ...[...positions.values()].map((p) => p.y + NODE_H)) + 80,
  })
</script>

<div class="min-w-0">
  <!-- Canvas header: the chain's name, its derived progress, its controls —
       the rail header's shape, so the two renders read as one surface. -->
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
    </span>
  </div>

  <!-- The canvas: wires UNDER the cards, cards above, ports on every card.
       Pan/zoom rides TALA-34's editor; here the canvas scrolls like the
       rails do, and the graph fits itself. -->
  <div class="mt-2 overflow-x-auto rounded-lg border border-line-subtle bg-canvas pb-1">
    <div class="relative" style="width: {canvasSize.w}px; height: {canvasSize.h}px">
      <svg class="pointer-events-none absolute inset-0 h-full w-full">
        {#each workchain.edges as e (e.fromTaskId + '>' + e.toTaskId)}
          {@const f = byTask.get(e.fromTaskId)}
          {@const t = byTask.get(e.toTaskId)}
          {#if f && t}
            {@const st = wireState(f, t)}
            <path
              d={wirePath(nodePos(e.fromTaskId), nodePos(e.toTaskId))}
              fill="none"
              stroke-width={st === 'idle' ? 1.5 : 2}
              class={cn(
                st === 'idle' && 'stroke-line-strong',
                st === 'fired' && 'stroke-accent',
                st === 'done' && 'stroke-success opacity-60',
              )}
              stroke-dasharray={st === 'fired' ? '6 3' : undefined}
            />
          {/if}
        {/each}
      </svg>
      {#each workchain.steps as step (step.taskId)}
        {@const pos = nodePos(step.taskId)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          role="button"
          tabindex={0}
          onclick={() => onOpen(step.taskId)}
          onkeydown={(e) => {
            if (e.key === 'Enter' && e.target === e.currentTarget) onOpen(step.taskId)
          }}
          onpointerdown={(e) => {
            if (e.button === 0) startDrag(step.taskId, e)
          }}
          onpointermove={moveDrag}
          onpointerup={endDrag}
          class={cn(
            'absolute cursor-grab rounded-lg border border-line bg-panel p-3 text-left transition-colors active:cursor-grabbing',
            step.state === 'head' && 'ring-1 ring-inset ring-accent',
            step.state === 'ready' && 'ring-1 ring-inset ring-accent',
            (step.state === 'done' || step.state === 'archived') && 'opacity-50',
          )}
          style="left: {pos.x}px; top: {pos.y}px; width: {NODE_W}px; min-height: {NODE_H}px"
        >
          <!-- Ports: the wire targets TALA-34's drags aim at. Left in, right
               out — real affordances, not decoration. -->
          <span
            data-port="in"
            class="absolute top-1/2 -left-1 h-2.5 w-2.5 -translate-y-1/2 rounded-full border border-line-strong bg-panel"
          ></span>
          <span
            data-port="out"
            class="absolute top-1/2 -right-1 h-2.5 w-2.5 -translate-y-1/2 rounded-full border border-line-strong bg-panel"
          ></span>
          <div class="flex items-center gap-1.5">
            <StatusDot color={statusColorOf(step.status, boardStatuses)} />
            {#if step.ticketRef}
              <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{step.ticketRef}</span>
            {/if}
            {#if step.state === 'done'}
              <Check size={12} class="ml-auto shrink-0 text-success" />
            {:else if step.state === 'head'}
              <span class="ml-auto font-mono text-[9px] uppercase tracking-[0.05em] text-accent">head</span>
            {:else if step.state === 'ready'}
              <span class="ml-auto font-mono text-[9px] uppercase tracking-[0.05em] text-accent">ready</span>
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
                {formatShortDate(step.dueDate)}
              </span>
            {/if}
            {#if step.assignees.length > 0}
              <span class="ml-auto flex -space-x-1.5">
                {#each info(step).slice(0, 3) as a (a.key)}
                  <Avatar name={a.label} class="h-4.5 w-4.5 ring-2 ring-[color:var(--theme-panel)]" />
                {/each}
              </span>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  </div>
</div>