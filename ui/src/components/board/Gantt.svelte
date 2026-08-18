<script lang="ts">
  // Gantt view — the board's tickets on a time axis. Bars run startDate → dueDate
  // (single-date tickets render a one-day bar), colored by status, draggable when
  // the viewer can edit: drag the body to shift both dates, drag an edge to move
  // just that end (day snapping, pointer-captured). Sub-tasks indent under their
  // parent. Below the chart sits the UNSCHEDULED list — drag a ticket up into
  // the chart and it schedules where you drop it. Zoom with the −/+ controls.
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Minus, Plus } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { listStagger } from '@/lib/motion'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { updateTask, type Board } from '@/lib/boards.svelte'
  import { LABEL_CSS, isOverdueTask } from './field-pills'
  import { statusColorOf, useBoardStatuses } from '@/lib/statuses'
  import type { Task } from '@/lib/task-const'

  const DAY = 24 * 60 * 60 * 1000
  const MIN_W = 8
  const MAX_W = 56

  const dayFloor = (t: number) => {
    const d = new Date(t)
    d.setHours(0, 0, 0, 0)
    return d.getTime()
  }

  const at17 = (t: number) => {
    const d = new Date(t)
    d.setHours(17, 0, 0, 0)
    return d.toISOString()
  }
  const at9 = (t: number) => {
    const d = new Date(t)
    d.setHours(9, 0, 0, 0)
    return d.toISOString()
  }

  interface Span {
    task: Task
    start: number // day-floored ms
    end: number // inclusive day-floored ms
    child: boolean
  }

  let { board, tasks, onOpen }: { board: Board; tasks: Task[]; onOpen: (id: string) => void } = $props()

  const qc = useQueryClient()
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  // `= []` again: on a failed read every bar falls back to the default status
  // palette and `isOverdueTask` loses the board's own terminal categories, so
  // finished work can render as overdue. Colour and lateness are statements
  // about this board's workflow — don't make them from a read that broke.
  const statusesQuery = useBoardStatuses(() => board.id)
  const boardStatuses = $derived(statusesQuery.data ?? [])
  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  let dayW = $state(26)

  // The grid always fills the viewport at the current zoom — zooming out
  // widens the RANGE, never leaves dead space past the last day.
  let scrollEl = $state<HTMLDivElement | null>(null)
  let containerW = $state(0)
  $effect(() => {
    const el = scrollEl
    if (!el) return
    const ro = new ResizeObserver(() => (containerW = el.clientWidth))
    ro.observe(el)
    containerW = el.clientWidth
    // Wheel = navigation: vertical zooms (anchored at the cursor so the day
    // under it stays put), horizontal pans natively. Native listener with
    // passive:false — preventDefault must actually work.
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return // horizontal: let it pan
      e.preventDefault()
      const cursorX = e.clientX - el.getBoundingClientRect().left
      // Proportional to wheel delta (clamped) — gentle on fine-grained
      // trackpads, controlled on notchy mouse wheels.
      const factor = Math.exp(-Math.max(-100, Math.min(100, e.deltaY)) * 0.0012)
      const w = dayW
      const next = Math.min(MAX_W, Math.max(MIN_W, w * factor))
      if (next !== w) {
        const day = (el.scrollLeft + cursorX - 220) / w
        requestAnimationFrame(() => {
          el.scrollLeft = Math.max(0, day * next - (cursorX - 220))
        })
      }
      dayW = next
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      ro.disconnect()
      el.removeEventListener('wheel', onWheel)
    }
  })

  const layout = $derived.by(() => {
    const scheduled = tasks.filter((t) => t.startDate || t.dueDate)
    const unscheduled = tasks.filter((t) => !t.startDate && !t.dueDate && !t.archivedAt)
    const spanOf = (t: Task): { start: number; end: number } => {
      const s = t.startDate ? dayFloor(Date.parse(t.startDate)) : null
      const e = t.dueDate ? dayFloor(Date.parse(t.dueDate)) : null
      return { start: s ?? e!, end: e ?? s! }
    }
    const parents = scheduled.filter((t) => !t.parentId).sort((a, b) => spanOf(a).start - spanOf(b).start)
    const rows: Span[] = []
    for (const p of parents) {
      rows.push({ task: p, ...spanOf(p), child: false })
      for (const c of scheduled.filter((t) => t.parentId === p.id).sort((a, b) => spanOf(a).start - spanOf(b).start)) {
        rows.push({ task: c, ...spanOf(c), child: true })
      }
    }
    for (const c of scheduled.filter((t) => t.parentId && !rows.some((r) => r.task.id === t.id))) {
      rows.push({ task: c, ...spanOf(c), child: false })
    }
    const today = dayFloor(Date.now())
    // Never a peephole: at least a week back and two months forward, growing
    // as scheduled work extends beyond that — and never NARROWER than the
    // viewport at this zoom (fill-to-container days appended at the end).
    const min = Math.min(today - 7 * DAY, ...rows.map((r) => r.start - 3 * DAY))
    const max = Math.max(today + 60 * DAY, ...rows.map((r) => r.end + 10 * DAY))
    const dataDays = Math.round((max - min) / DAY) + 1
    const fillDays = containerW > 0 ? Math.ceil((containerW - 220) / dayW) + 1 : 0
    return { spans: rows, unscheduled, rangeStart: min, days: Math.max(dataDays, fillDays) }
  })
  const spans = $derived(layout.spans)
  const unscheduled = $derived(layout.unscheduled)
  const rangeStart = $derived(layout.rangeStart)
  const days = $derived(layout.days)

  const x = (t: number) => ((t - rangeStart) / DAY) * dayW
  const today = dayFloor(Date.now())
  const labelW = 220

  // ── Bar dragging (pointer-captured; click-after-drag suppressed) ─────────
  let drag: { id: string; mode: 'move' | 'start' | 'end'; originX: number; s: number; e: number; moved: boolean } | null = null
  // Survives past pointerup: the browser fires click AFTER pointerup, so the
  // "was this a drag?" answer must outlive the drag state itself.
  let justDragged = false
  let preview = $state<{ id: string; s: number; e: number } | null>(null)

  const applyDelta = (d: NonNullable<typeof drag>, deltaDays: number) => {
    let s = d.s
    let en = d.e
    if (d.mode === 'move') {
      s += deltaDays * DAY
      en += deltaDays * DAY
    } else if (d.mode === 'start') {
      s = Math.min(d.s + deltaDays * DAY, en)
    } else {
      en = Math.max(d.e + deltaDays * DAY, s)
    }
    return { s, en }
  }

  const beginDrag = (e: PointerEvent, span: Span, mode: 'move' | 'start' | 'end') => {
    if (!canEdit) return
    e.preventDefault()
    e.stopPropagation()
    ;(e.target as Element).setPointerCapture?.(e.pointerId)
    justDragged = false
    drag = { id: span.task.id, mode, originX: e.clientX, s: span.start, e: span.end, moved: false }
  }
  const onPointerMove = (e: PointerEvent) => {
    const d = drag
    if (!d) return
    if (Math.abs(e.clientX - d.originX) > 3) justDragged = true
    const deltaDays = Math.round((e.clientX - d.originX) / dayW)
    if (deltaDays !== 0) d.moved = true
    const { s, en } = applyDelta(d, deltaDays)
    preview = { id: d.id, s, e: en }
  }
  const onPointerUp = (e: PointerEvent) => {
    const d = drag
    drag = null
    preview = null
    if (!d) return
    const deltaDays = Math.round((e.clientX - d.originX) / dayW)
    if (deltaDays === 0) return
    const { s, en } = applyDelta(d, deltaDays)
    void updateTask(d.id, { startDate: at9(s), dueDate: at17(en) }).then(invalidate)
  }

  // ── Drop-to-schedule from the unscheduled list ───────────────────────────
  let rowsEl = $state<HTMLDivElement | null>(null)
  let dropDay = $state<number | null>(null)
  const dayAt = (clientX: number): number | null => {
    const rect = rowsEl?.getBoundingClientRect()
    if (!rect) return null
    const i = Math.floor((clientX - rect.left - labelW) / dayW)
    return i >= 0 && i < days ? rangeStart + i * DAY : null
  }
  const onChartDragOver = (e: DragEvent) => {
    if (!canEdit || !e.dataTransfer?.types.includes('text/gantt-task')) return
    e.preventDefault()
    dropDay = dayAt(e.clientX)
  }
  const onChartDrop = (e: DragEvent) => {
    e.preventDefault()
    const id = e.dataTransfer?.getData('text/gantt-task')
    const day = dayAt(e.clientX)
    dropDay = null
    if (!id || day === null) return
    // Land where dropped: start = the day under the cursor, due 2 days later.
    void updateTask(id, { startDate: at9(day), dueDate: at17(day + 2 * DAY) }).then(invalidate)
  }

  // Month header cells across the range.
  const months = $derived.by(() => {
    const out: Array<{ label: string; from: number; daysIn: number }> = []
    let cur = new Date(rangeStart)
    cur.setDate(1)
    while (cur.getTime() < rangeStart + days * DAY) {
      const from = Math.max(cur.getTime(), rangeStart)
      const next = new Date(cur.getFullYear(), cur.getMonth() + 1, 1).getTime()
      const to = Math.min(next, rangeStart + days * DAY)
      out.push({
        label: cur.toLocaleDateString(undefined, { month: 'short', year: cur.getMonth() === 0 ? 'numeric' : undefined }),
        from,
        daysIn: Math.round((to - from) / DAY),
      })
      cur = new Date(next)
    }
    return out
  })

  const zoom = (dir: 1 | -1) => {
    dayW = Math.min(MAX_W, Math.max(MIN_W, dayW * (dir === 1 ? 1.4 : 1 / 1.4)))
  }
</script>

<div class="flex h-full flex-col">
  <!-- The chart itself is the tickets, which loaded — so the timeline stays
      and the failed workflow read is marked rather than replacing it. -->
  {#if statusesQuery.isError}
    <QueryError
      variant="inline"
      class="border-b border-line-subtle px-4 py-2"
      title={statusesQuery.data === undefined
        ? 'Could not load this board’s statuses — bar colours and overdue marks are guesses'
        : 'Statuses may be out of date'}
      error={statusesQuery.error}
      onRetry={() => void statusesQuery.refetch()}
    />
  {/if}
  <div bind:this={scrollEl} class="min-h-0 flex-1 overflow-auto">
    <div class="flex min-h-full flex-col" style="min-width: {labelW + days * dayW}px">
      <!-- ── Header: months + day grid + zoom ── -->
      <div class="sticky top-0 z-10 flex border-b border-line bg-surface/95 backdrop-blur">
        <div style="width: {labelW}px" class="flex shrink-0 items-center gap-0.5 border-r border-line-subtle px-2">
          <button data-dither-fill onclick={() => zoom(-1)} title="Zoom out" class="grid h-5 w-5 place-items-center rounded text-muted transition-colors hover:text-fg">
            <Minus size={12} />
          </button>
          <button data-dither-fill onclick={() => zoom(1)} title="Zoom in" class="grid h-5 w-5 place-items-center rounded text-muted transition-colors hover:text-fg">
            <Plus size={12} />
          </button>
          <span class="ml-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Zoom</span>
        </div>
        <div class="relative">
          <div class="flex">
            {#each months as m (m.from)}
              <div
                style="width: {m.daysIn * dayW}px"
                class="overflow-hidden border-r border-line-subtle px-2 py-1 font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted"
              >
                {m.label}
              </div>
            {/each}
          </div>
          <div class="flex border-t border-line-subtle">
            {#each { length: days } as _, i (i)}
              {@const d = new Date(rangeStart + i * DAY)}
              {@const wk = d.getDay() === 0 || d.getDay() === 6}
              <div
                style="width: {dayW}px"
                class={cn('overflow-hidden py-0.5 text-center font-mono text-[9px]', wk ? 'bg-panel/70 text-muted/60' : 'text-muted', d.getTime() === today && 'font-bold text-accent')}
              >
                {dayW >= 14 ? d.getDate() : dayW >= 9 && d.getDate() % 2 === 1 ? d.getDate() : ''}
              </div>
            {/each}
          </div>
        </div>
      </div>

      <!-- ── Rows (also the drop target for scheduling) ── -->
      <div bind:this={rowsEl} use:listStagger class="relative flex-1" ondragover={onChartDragOver} ondragleave={() => (dropDay = null)} ondrop={onChartDrop}>
        <!-- Grid scaffolding (weekend shading, today line) is not a row — the
             cascade skips it so the chart frame never shifts. -->
        <div data-no-stagger class="pointer-events-none absolute inset-y-0" style="left: {labelW}px">
          {#each { length: days } as _, i (i)}
            {@const d = new Date(rangeStart + i * DAY)}
            {@const wk = d.getDay() === 0 || d.getDay() === 6}
            {#if wk}
              <div class="absolute inset-y-0 bg-panel/50" style="left: {i * dayW}px; width: {dayW}px"></div>
            {/if}
          {/each}
          <div class="absolute inset-y-0 w-px bg-accent" style="left: {x(today) + dayW / 2}px" title="Today"></div>
          {#if dropDay !== null}
            <div class="absolute inset-y-0 rounded-sm bg-accent/15 ring-1 ring-accent" style="left: {x(dropDay)}px; width: {dayW * 3}px"></div>
          {/if}
        </div>

        {#if spans.length === 0}
          <div class="px-6 py-10 font-sans text-sm text-muted">
            Nothing scheduled yet — set start/due dates, or drag a ticket up from the list below and drop it on a day.
          </div>
        {/if}
        {#each spans as r (r.task.id)}
          {@const p = preview?.id === r.task.id ? preview : null}
          {@const s = p?.s ?? r.start}
          {@const e = p?.e ?? r.end}
          {@const late = isOverdueTask(r.task, boardStatuses)}
          <!-- Ticket color wins; status tint is the fallback. -->
          {@const c = r.task.color ? LABEL_CSS[r.task.color] : statusColorOf(r.task.status, boardStatuses)}
          <div data-dither-fill class="group flex h-12 items-center border-b border-line-subtle/60 transition-colors/40">
            <button
              onclick={() => onOpen(r.task.id)}
              style="width: {labelW}px"
              class={cn('flex h-full shrink-0 items-center gap-1.5 truncate border-r border-line-subtle px-2 text-left text-xs transition-colors hover:text-fg', r.child ? 'pl-6 text-muted' : 'text-fg')}
            >
              {#if r.task.ticketRef}
                <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{r.task.ticketRef}</span>
              {/if}
              <span class="truncate font-sans">{r.task.title}</span>
            </button>
            <div class="relative h-full flex-1">
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <div
                onpointerdown={(ev) => beginDrag(ev, r, 'move')}
                onpointermove={onPointerMove}
                onpointerup={onPointerUp}
                onclick={(ev) => {
                  // A completed drag must not read as a click — click
                  // fires AFTER pointerup, so check the surviving flag.
                  ev.stopPropagation()
                  if (justDragged) {
                    justDragged = false
                    return
                  }
                  onOpen(r.task.id)
                }}
                title={`${r.task.title} — drag to reschedule`}
                class={cn('absolute top-2 flex h-8 touch-none select-none items-center gap-1 rounded-md border px-2.5 font-sans text-[11px] text-fg', canEdit ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer', late && 'ring-1 ring-danger')}
                style="left: {x(s)}px; width: {Math.max(dayW, x(e) - x(s) + dayW)}px; background: color-mix(in srgb, {c} {r.task.color ? 30 : 22}%, transparent); border-color: color-mix(in srgb, {c} {r.task.color ? 70 : 55}%, transparent)"
              >
                {#if canEdit}
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <span
                    onpointerdown={(ev) => beginDrag(ev, r, 'start')}
                    onpointermove={onPointerMove}
                    onpointerup={onPointerUp}
                    title="Drag to change the start date"
                    class="group/h absolute -left-2 inset-y-0 flex w-5 cursor-ew-resize items-center justify-center"
                  >
                    <span class="h-4 w-0.5 rounded-full bg-fg/0 transition-colors group-hover/h:bg-fg/50"></span>
                  </span>
                {/if}
                <span class="pointer-events-none truncate">{r.task.title}</span>
                {#if canEdit}
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <span
                    onpointerdown={(ev) => beginDrag(ev, r, 'end')}
                    onpointermove={onPointerMove}
                    onpointerup={onPointerUp}
                    title="Drag to change the due date"
                    class="group/h absolute -right-2 inset-y-0 flex w-5 cursor-ew-resize items-center justify-center"
                  >
                    <span class="h-4 w-0.5 rounded-full bg-fg/0 transition-colors group-hover/h:bg-fg/50"></span>
                  </span>
                {/if}
              </div>
            </div>
          </div>
        {/each}
      </div>
    </div>
  </div>

  <!-- ── Unscheduled list: a real list, draggable into the chart ── -->
  {#if unscheduled.length > 0}
    <div class="max-h-48 shrink-0 overflow-y-auto border-t border-line">
      <div class="sticky top-0 flex items-center gap-2 bg-surface/95 px-4 pb-1 pt-3 backdrop-blur">
        <span class="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-ink-dim">Unscheduled ({unscheduled.length})</span>
        {#if canEdit}<span class="font-sans text-[10px] text-muted">drag a row onto a day above to schedule it</span>{/if}
      </div>
      <ul class="divide-y divide-line-subtle/60 px-2 pb-2" use:listStagger>
        {#each unscheduled as t (t.id)}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
          <li data-dither-fill
            draggable={canEdit}
            ondragstart={(e) => {
              e.dataTransfer?.setData('text/gantt-task', t.id)
              if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
            }}
            onclick={() => onOpen(t.id)}
            class={cn('flex items-center gap-2 rounded-md px-2 py-1.5 text-xs transition-colors', canEdit ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer')}
          >
            <span class="h-1.5 w-1.5 shrink-0 rounded-full" style:background={t.color ? LABEL_CSS[t.color] : statusColorOf(t.status, boardStatuses)}></span>
            {#if t.ticketRef}
              <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.ticketRef}</span>
            {/if}
            <span class="min-w-0 flex-1 truncate font-sans text-fg">{t.title}</span>
            {#if t.estimatedHours != null}
              <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.estimatedHours}h</span>
            {/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>
