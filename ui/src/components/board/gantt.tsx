// Gantt view — the board's tickets on a time axis. Bars run startDate → dueDate
// (single-date tickets render a one-day bar), colored by status, draggable when
// the viewer can edit: drag the body to shift both dates, drag an edge to move
// just that end (day snapping). Sub-tasks indent under their parent; tickets
// with no dates sit on an "Unscheduled" shelf with a one-click schedule.
import { useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { cn } from '@/lib/cn'
import { updateTask, type Board } from '@/lib/boards'
import { STATUS_COLOR, isOverdueTask } from '@/components/board/field-pills'
import type { Task } from '@/lib/task-const'

const DAY = 24 * 60 * 60 * 1000
const DAY_W = 26 // px per day

const dayFloor = (t: number) => {
  const d = new Date(t)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
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
  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })

  const { spans, unscheduled, rangeStart, days } = useMemo(() => {
    const scheduled = tasks.filter((t) => t.startDate || t.dueDate)
    const unscheduled = tasks.filter((t) => !t.startDate && !t.dueDate && !t.archivedAt)
    // Parents first, their children directly beneath; then the rest by start.
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
    // Children whose parent is unscheduled still show, un-indented.
    for (const c of scheduled.filter((t) => t.parentId && !rows.some((r) => r.task.id === t.id))) {
      rows.push({ task: c, ...spanOf(c), child: false })
    }
    const today = dayFloor(Date.now())
    // Never a peephole: at least a week back and two months forward, growing
    // as scheduled work extends beyond that.
    const min = Math.min(today - 7 * DAY, ...rows.map((r) => r.start - 3 * DAY))
    const max = Math.max(today + 60 * DAY, ...rows.map((r) => r.end + 10 * DAY))
    return { spans: rows, unscheduled, rangeStart: min, days: Math.round((max - min) / DAY) + 1 }
  }, [tasks])

  const x = (t: number) => ((t - rangeStart) / DAY) * DAY_W
  const today = dayFloor(Date.now())

  // ── Drag state: move whole bar, or one edge ────────────────────────────
  const dragRef = useRef<{ id: string; mode: 'move' | 'start' | 'end'; originX: number; s: number; e: number } | null>(null)
  const [preview, setPreview] = useState<{ id: string; s: number; e: number } | null>(null)

  const beginDrag = (e: React.MouseEvent, span: Span, mode: 'move' | 'start' | 'end') => {
    if (!canEdit) return
    e.preventDefault()
    e.stopPropagation()
    dragRef.current = { id: span.task.id, mode, originX: e.clientX, s: span.start, e: span.end }
    const onMove = (ev: MouseEvent) => {
      const d = dragRef.current
      if (!d) return
      const deltaDays = Math.round((ev.clientX - d.originX) / DAY_W)
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
      setPreview({ id: d.id, s, e: en })
    }
    const onUp = (ev: MouseEvent) => {
      document.removeEventListener('mousemove', onMove)
      document.removeEventListener('mouseup', onUp)
      const d = dragRef.current
      dragRef.current = null
      setPreview(null)
      if (!d) return
      const deltaDays = Math.round((ev.clientX - d.originX) / DAY_W)
      if (deltaDays === 0) return
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
      const at17 = (t: number) => {
        const dd = new Date(t)
        dd.setHours(17, 0, 0, 0)
        return dd.toISOString()
      }
      void updateTask(d.id, { startDate: at17(s), dueDate: at17(en) }).then(invalidate)
    }
    document.addEventListener('mousemove', onMove)
    document.addEventListener('mouseup', onUp)
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

  const schedule = async (t: Task) => {
    const s = new Date()
    s.setHours(9, 0, 0, 0)
    const e = new Date(Date.now() + 2 * DAY)
    e.setHours(17, 0, 0, 0)
    await updateTask(t.id, { startDate: s.toISOString(), dueDate: e.toISOString() })
    invalidate()
  }

  const labelW = 220
  return (
    <div className="flex h-full flex-col overflow-auto">
      <div style={{ minWidth: labelW + days * DAY_W }}>
        {/* ── Header: months + day grid ── */}
        <div className="sticky top-0 z-10 flex border-b border-line bg-surface/95 backdrop-blur">
          <div style={{ width: labelW }} className="shrink-0 border-r border-line-subtle" />
          <div className="relative">
            <div className="flex">
              {months.map((m) => (
                <div
                  key={m.from}
                  style={{ width: m.daysIn * DAY_W }}
                  className="border-r border-line-subtle px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-muted"
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
                    style={{ width: DAY_W }}
                    className={cn('py-0.5 text-center text-[9px]', wk ? 'bg-sidebar/70 text-muted/60' : 'text-muted', d.getTime() === today && 'font-bold text-accent')}
                  >
                    {d.getDate()}
                  </div>
                )
              })}
            </div>
          </div>
        </div>

        {/* ── Rows ── */}
        <div className="relative">
          {/* weekend + today verticals across all rows */}
          <div className="pointer-events-none absolute inset-y-0" style={{ left: labelW }}>
            {Array.from({ length: days }, (_, i) => {
              const d = new Date(rangeStart + i * DAY)
              const wk = d.getDay() === 0 || d.getDay() === 6
              return wk ? (
                <div key={i} className="absolute inset-y-0 bg-sidebar/50" style={{ left: i * DAY_W, width: DAY_W }} />
              ) : null
            })}
            <div className="absolute inset-y-0 w-px bg-accent" style={{ left: x(today) + DAY_W / 2 }} title="Today" />
          </div>

          {spans.length === 0 && (
            <div className="px-6 py-10 text-sm text-muted">Nothing scheduled yet — set start/due dates, or drag tickets up from the shelf below.</div>
          )}
          {spans.map((r) => {
            const p = preview?.id === r.task.id ? preview : null
            const s = p?.s ?? r.start
            const e = p?.e ?? r.end
            const late = isOverdueTask(r.task)
            return (
              <div key={r.task.id} className="group flex h-9 items-center border-b border-line-subtle/60 hover:bg-card/40">
                <button
                  onClick={() => onOpen(r.task.id)}
                  style={{ width: labelW }}
                  className={cn('flex h-full shrink-0 items-center gap-1.5 truncate border-r border-line-subtle px-2 text-left text-xs transition-colors hover:text-fg', r.child ? 'pl-6 text-muted' : 'text-fg')}
                >
                  {r.task.ticketRef && <span className="shrink-0 font-[var(--font-mono)] text-[10px] text-muted">{r.task.ticketRef}</span>}
                  <span className="truncate font-sans">{r.task.title}</span>
                </button>
                <div className="relative h-full flex-1">
                  <div
                    onMouseDown={(ev) => beginDrag(ev, r, 'move')}
                    onClick={() => !dragRef.current && onOpen(r.task.id)}
                    title={`${r.task.title} — drag to reschedule`}
                    className={cn('absolute top-1.5 flex h-6 items-center gap-1 overflow-hidden rounded-md border px-1.5 text-[10px] text-fg', canEdit ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer', late && 'ring-1 ring-[color:var(--theme-danger)]')}
                    style={{
                      left: x(s),
                      width: Math.max(DAY_W, x(e) - x(s) + DAY_W),
                      background: `color-mix(in srgb, ${STATUS_COLOR[r.task.status]} 22%, transparent)`,
                      borderColor: `color-mix(in srgb, ${STATUS_COLOR[r.task.status]} 55%, transparent)`,
                    }}
                  >
                    {canEdit && (
                      <span onMouseDown={(ev) => beginDrag(ev, r, 'start')} className="absolute inset-y-0 left-0 w-1.5 cursor-ew-resize" />
                    )}
                    <span className="truncate">{r.task.title}</span>
                    {canEdit && (
                      <span onMouseDown={(ev) => beginDrag(ev, r, 'end')} className="absolute inset-y-0 right-0 w-1.5 cursor-ew-resize" />
                    )}
                  </div>
                </div>
              </div>
            )
          })}
        </div>
      </div>

      {/* ── Unscheduled shelf ── */}
      {unscheduled.length > 0 && (
        <div className="border-t border-line px-4 py-3">
          <div className="mb-2 text-[10px] font-semibold uppercase tracking-wide text-muted">Unscheduled ({unscheduled.length})</div>
          <div className="flex flex-wrap gap-1.5">
            {unscheduled.map((t) => (
              <span key={t.id} className="inline-flex items-center gap-1 rounded-lg border border-line-subtle px-2 py-1 text-xs">
                <button onClick={() => onOpen(t.id)} className="text-muted transition-colors hover:text-fg">
                  {t.ticketRef ? `${t.ticketRef} ` : ''}
                  {t.title}
                </button>
                {canEdit && (
                  <button
                    onClick={() => void schedule(t)}
                    title="Schedule: start today, due in 2 days"
                    className="text-muted transition-colors hover:text-accent"
                  >
                    →
                  </button>
                )}
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
