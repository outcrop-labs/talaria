// Gantt view — the board's tickets on a time axis. Bars run startDate → dueDate
// (single-date tickets render a one-day bar), colored by status, draggable when
// the viewer can edit: drag the body to shift both dates, drag an edge to move
// just that end (day snapping, pointer-captured). Sub-tasks indent under their
// parent. Below the chart sits the UNSCHEDULED list — drag a ticket up into
// the chart and it schedules where you drop it. Zoom with the −/+ controls.
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Minus, Plus } from 'lucide-react'
import { cn } from '@/lib/cn'
import { updateTask, type Board } from '@/lib/boards'
import { LABEL_CSS, isOverdueTask } from '@/components/board/field-pills'
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

export function Gantt({ board, tasks, onOpen }: { board: Board; tasks: Task[]; onOpen: (id: string) => void }) {
  const qc = useQueryClient()
  const canEdit = board.role === 'owner' || board.role === 'editor'
  const { data: boardStatuses = [] } = useBoardStatuses(board.id)
  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  const [dayW, setDayW] = useState(26)

  // The grid always fills the viewport at the current zoom — zooming out
  // widens the RANGE, never leaves dead space past the last day.
  const scrollRef = useRef<HTMLDivElement>(null)
  const [containerW, setContainerW] = useState(0)
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setContainerW(el.clientWidth))
    ro.observe(el)
    setContainerW(el.clientWidth)
    // Wheel = navigation: vertical zooms (anchored at the cursor so the day
    // under it stays put), horizontal pans natively. Native listener with
    // passive:false — preventDefault must actually work.
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return // horizontal: let it pan
      e.preventDefault()
      const cursorX = e.clientX - el.getBoundingClientRect().left
      setDayW((w) => {
        // Proportional to wheel delta (clamped) — gentle on fine-grained
        // trackpads, controlled on notchy mouse wheels.
        const factor = Math.exp(-Math.max(-100, Math.min(100, e.deltaY)) * 0.0012)
        const next = Math.min(MAX_W, Math.max(MIN_W, w * factor))
        if (next !== w) {
          const day = (el.scrollLeft + cursorX - 220) / w
          requestAnimationFrame(() => {
            el.scrollLeft = Math.max(0, day * next - (cursorX - 220))
          })
        }
        return next
      })
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      ro.disconnect()
      el.removeEventListener('wheel', onWheel)
    }
  }, [])

  const { spans, unscheduled, rangeStart, days } = useMemo(() => {
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
  }, [tasks, containerW, dayW])

  const x = (t: number) => ((t - rangeStart) / DAY) * dayW
  const today = dayFloor(Date.now())
  const labelW = 220

  // ── Bar dragging (pointer-captured; click-after-drag suppressed) ─────────
  const dragRef = useRef<{ id: string; mode: 'move' | 'start' | 'end'; originX: number; s: number; e: number; moved: boolean } | null>(null)
  // Survives past pointerup: the browser fires click AFTER pointerup, so the
  // "was this a drag?" answer must outlive the drag state itself.
  const justDraggedRef = useRef(false)
  const [preview, setPreview] = useState<{ id: string; s: number; e: number } | null>(null)

  const applyDelta = (d: NonNullable<typeof dragRef.current>, deltaDays: number) => {
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

  const beginDrag = (e: React.PointerEvent, span: Span, mode: 'move' | 'start' | 'end') => {
    if (!canEdit) return
    e.preventDefault()
    e.stopPropagation()
    ;(e.target as Element).setPointerCapture?.(e.pointerId)
    justDraggedRef.current = false
    dragRef.current = { id: span.task.id, mode, originX: e.clientX, s: span.start, e: span.end, moved: false }
  }
  const onPointerMove = (e: React.PointerEvent) => {
    const d = dragRef.current
    if (!d) return
    if (Math.abs(e.clientX - d.originX) > 3) justDraggedRef.current = true
    const deltaDays = Math.round((e.clientX - d.originX) / dayW)
    if (deltaDays !== 0) d.moved = true
    const { s, en } = applyDelta(d, deltaDays)
    setPreview({ id: d.id, s, e: en })
  }
  const onPointerUp = (e: React.PointerEvent) => {
    const d = dragRef.current
    dragRef.current = null
    setPreview(null)
    if (!d) return
    const deltaDays = Math.round((e.clientX - d.originX) / dayW)
    if (deltaDays === 0) return
    const { s, en } = applyDelta(d, deltaDays)
    void updateTask(d.id, { startDate: at9(s), dueDate: at17(en) }).then(invalidate)
  }

  // ── Drop-to-schedule from the unscheduled list ───────────────────────────
  const rowsRef = useRef<HTMLDivElement>(null)
  const [dropDay, setDropDay] = useState<number | null>(null)
  const dayAt = (clientX: number): number | null => {
    const rect = rowsRef.current?.getBoundingClientRect()
    if (!rect) return null
    const i = Math.floor((clientX - rect.left - labelW) / dayW)
    return i >= 0 && i < days ? rangeStart + i * DAY : null
  }
  const onChartDragOver = (e: React.DragEvent) => {
    if (!canEdit || !e.dataTransfer.types.includes('text/gantt-task')) return
    e.preventDefault()
    setDropDay(dayAt(e.clientX))
  }
  const onChartDrop = (e: React.DragEvent) => {
    e.preventDefault()
    const id = e.dataTransfer.getData('text/gantt-task')
    const day = dayAt(e.clientX)
    setDropDay(null)
    if (!id || day === null) return
    // Land where dropped: start = the day under the cursor, due 2 days later.
    void updateTask(id, { startDate: at9(day), dueDate: at17(day + 2 * DAY) }).then(invalidate)
  }

  // Month header cells across the range.
  const months = useMemo(() => {
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
  }, [rangeStart, days])

  const zoom = (dir: 1 | -1) => setDayW((w) => Math.min(MAX_W, Math.max(MIN_W, w * (dir === 1 ? 1.4 : 1 / 1.4))))

  return (
    <div className="flex h-full flex-col">
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto">
        <div className="flex min-h-full flex-col" style={{ minWidth: labelW + days * dayW }}>
          {/* ── Header: months + day grid + zoom ── */}
          <div className="sticky top-0 z-10 flex border-b border-line bg-surface/95 backdrop-blur">
            <div style={{ width: labelW }} className="flex shrink-0 items-center gap-0.5 border-r border-line-subtle px-2">
              <button onClick={() => zoom(-1)} title="Zoom out" className="grid h-5 w-5 place-items-center rounded text-muted transition-colors hover:bg-hover hover:text-fg">
                <Minus size={12} />
              </button>
              <button onClick={() => zoom(1)} title="Zoom in" className="grid h-5 w-5 place-items-center rounded text-muted transition-colors hover:bg-hover hover:text-fg">
                <Plus size={12} />
              </button>
              <span className="ml-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Zoom</span>
            </div>
            <div className="relative">
              <div className="flex">
                {months.map((m) => (
                  <div
                    key={m.from}
                    style={{ width: m.daysIn * dayW }}
                    className="overflow-hidden border-r border-line-subtle px-2 py-1 font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted"
                  >
                    {m.label}
                  </div>
                ))}
              </div>
              <div className="flex border-t border-line-subtle">
                {Array.from({ length: days }, (_, i) => {
                  const d = new Date(rangeStart + i * DAY)
                  const wk = d.getDay() === 0 || d.getDay() === 6
                  return (
                    <div
                      key={i}
                      style={{ width: dayW }}
                      className={cn('overflow-hidden py-0.5 text-center font-mono text-[9px]', wk ? 'bg-panel/70 text-muted/60' : 'text-muted', d.getTime() === today && 'font-bold text-accent')}
                    >
                      {dayW >= 14 ? d.getDate() : dayW >= 9 && d.getDate() % 2 === 1 ? d.getDate() : ''}
                    </div>
                  )
                })}
              </div>
            </div>
          </div>

          {/* ── Rows (also the drop target for scheduling) ── */}
          <div ref={rowsRef} className="relative flex-1" onDragOver={onChartDragOver} onDragLeave={() => setDropDay(null)} onDrop={onChartDrop}>
            <div className="pointer-events-none absolute inset-y-0" style={{ left: labelW }}>
              {Array.from({ length: days }, (_, i) => {
                const d = new Date(rangeStart + i * DAY)
                const wk = d.getDay() === 0 || d.getDay() === 6
                return wk ? (
                  <div key={i} className="absolute inset-y-0 bg-panel/50" style={{ left: i * dayW, width: dayW }} />
                ) : null
              })}
              <div className="absolute inset-y-0 w-px bg-accent" style={{ left: x(today) + dayW / 2 }} title="Today" />
              {dropDay !== null && (
                <div className="absolute inset-y-0 rounded-sm bg-accent/15 ring-1 ring-accent" style={{ left: x(dropDay), width: dayW * 3 }} />
              )}
            </div>

            {spans.length === 0 && (
              <div className="px-6 py-10 font-sans text-sm text-muted">
                Nothing scheduled yet — set start/due dates, or drag a ticket up from the list below and drop it on a day.
              </div>
            )}
            {spans.map((r) => {
              const p = preview?.id === r.task.id ? preview : null
              const s = p?.s ?? r.start
              const e = p?.e ?? r.end
              const late = isOverdueTask(r.task, boardStatuses)
              return (
                <div key={r.task.id} className="group flex h-12 items-center border-b border-line-subtle/60 transition-colors hover:bg-hover/40">
                  <button
                    onClick={() => onOpen(r.task.id)}
                    style={{ width: labelW }}
                    className={cn('flex h-full shrink-0 items-center gap-1.5 truncate border-r border-line-subtle px-2 text-left text-xs transition-colors hover:text-fg', r.child ? 'pl-6 text-muted' : 'text-fg')}
                  >
                    {r.task.ticketRef && <span className="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{r.task.ticketRef}</span>}
                    <span className="truncate font-sans">{r.task.title}</span>
                  </button>
                  <div className="relative h-full flex-1">
                    <div
                      onPointerDown={(ev) => beginDrag(ev, r, 'move')}
                      onPointerMove={onPointerMove}
                      onPointerUp={onPointerUp}
                      onClick={(ev) => {
                        // A completed drag must not read as a click — click
                        // fires AFTER pointerup, so check the surviving flag.
                        ev.stopPropagation()
                        if (justDraggedRef.current) {
                          justDraggedRef.current = false
                          return
                        }
                        onOpen(r.task.id)
                      }}
                      title={`${r.task.title} — drag to reschedule`}
                      className={cn('absolute top-2 flex h-8 touch-none select-none items-center gap-1 rounded-md border px-2.5 font-sans text-[11px] text-fg', canEdit ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer', late && 'ring-1 ring-danger')}
                      style={(() => {
                        // Ticket color wins; status tint is the fallback.
                        const c = r.task.color ? LABEL_CSS[r.task.color] : statusColorOf(r.task.status, boardStatuses)
                        return {
                          left: x(s),
                          width: Math.max(dayW, x(e) - x(s) + dayW),
                          background: `color-mix(in srgb, ${c} ${r.task.color ? 30 : 22}%, transparent)`,
                          borderColor: `color-mix(in srgb, ${c} ${r.task.color ? 70 : 55}%, transparent)`,
                        }
                      })()}
                    >
                      {canEdit && (
                        <span
                          onPointerDown={(ev) => beginDrag(ev, r, 'start')}
                          onPointerMove={onPointerMove}
                          onPointerUp={onPointerUp}
                          title="Drag to change the start date"
                          className="group/h absolute -left-2 inset-y-0 flex w-5 cursor-ew-resize items-center justify-center"
                        >
                          <span className="h-4 w-0.5 rounded-full bg-fg/0 transition-colors group-hover/h:bg-fg/50" />
                        </span>
                      )}
                      <span className="pointer-events-none truncate">{r.task.title}</span>
                      {canEdit && (
                        <span
                          onPointerDown={(ev) => beginDrag(ev, r, 'end')}
                          onPointerMove={onPointerMove}
                          onPointerUp={onPointerUp}
                          title="Drag to change the due date"
                          className="group/h absolute -right-2 inset-y-0 flex w-5 cursor-ew-resize items-center justify-center"
                        >
                          <span className="h-4 w-0.5 rounded-full bg-fg/0 transition-colors group-hover/h:bg-fg/50" />
                        </span>
                      )}
                    </div>
                  </div>
                </div>
              )
            })}
          </div>
        </div>
      </div>

      {/* ── Unscheduled list: a real list, draggable into the chart ── */}
      {unscheduled.length > 0 && (
        <div className="max-h-48 shrink-0 overflow-y-auto border-t border-line">
          <div className="sticky top-0 flex items-center gap-2 bg-surface/95 px-4 pb-1 pt-3 backdrop-blur">
            <span className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-ink-dim">Unscheduled ({unscheduled.length})</span>
            {canEdit && <span className="font-sans text-[10px] text-muted">drag a row onto a day above to schedule it</span>}
          </div>
          <ul className="divide-y divide-line-subtle/60 px-2 pb-2">
            {unscheduled.map((t) => (
              <li
                key={t.id}
                draggable={canEdit}
                onDragStart={(e) => {
                  e.dataTransfer.setData('text/gantt-task', t.id)
                  e.dataTransfer.effectAllowed = 'move'
                }}
                onClick={() => onOpen(t.id)}
                className={cn('flex items-center gap-2 rounded-md px-2 py-1.5 text-xs transition-colors hover:bg-hover', canEdit ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer')}
              >
                <span className="h-1.5 w-1.5 shrink-0 rounded-full" style={{ background: t.color ? LABEL_CSS[t.color] : statusColorOf(t.status, boardStatuses) }} />
                {t.ticketRef && <span className="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.ticketRef}</span>}
                <span className="min-w-0 flex-1 truncate font-sans text-fg">{t.title}</span>
                {t.estimatedHours != null && <span className="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.estimatedHours}h</span>}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  )
}
