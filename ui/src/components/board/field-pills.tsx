// Ticket property pills — the ClickUp-style direct-manipulation layer, done
// the Mercury way. Each pill shows the value quietly, lights up on hover
// (FieldPill), and opens its picker in place (DropdownMenu). One set of
// components serves kanban cards, list rows, and group headers, so every
// surface manipulates tickets identically. All pickers stop propagation —
// the row/card click still opens the ticket.
import { CalendarDays, Flag, Timer, UserRound } from 'lucide-react'
import { Avatar } from '@/components/ui/avatar'
import { DropdownMenu, type ContextMenuEntry } from '@/components/ui/context-menu'
import { FieldPill } from '@/components/ui/field-pill'
import { cn } from '@/lib/cn'
import { assigneeInfo, userAssignee } from '@/lib/assignees'
import type { BoardMember } from '@/lib/boards'
import {
  PRIORITIES,
  PRIORITY_COLOR,
  STATUS_LABEL,
  TASK_STATUSES,
  type Priority,
  type Task,
  type TaskStatus,
} from '@/lib/task-const'

export type TicketPatch = {
  status?: TaskStatus
  priority?: Priority
  dueDate?: string | null
  estimatedHours?: number | null
  assignees?: string[]
}

export interface PillCtx {
  canEdit: boolean
  onPatch: (p: TicketPatch) => void
  agents: Array<{ id: string; label: string }>
  members: BoardMember[]
  meId?: string | null
}

export const STATUS_COLOR: Record<string, string> = {
  inbox: 'var(--theme-muted)',
  assigned: 'var(--theme-accent)',
  in_progress: 'var(--theme-warning)',
  blocked: 'var(--theme-danger)',
  quality_review: 'var(--theme-accent-secondary)',
  done: 'var(--theme-success)',
  failed: 'var(--theme-danger)',
  cancelled: 'var(--theme-muted)',
}

export const isOverdueTask = (t: Pick<Task, 'dueDate' | 'status'>) =>
  !!t.dueDate && new Date(t.dueDate).getTime() < Date.now() && !['done', 'cancelled'].includes(t.status)

const fmtDue = (iso: string) => {
  const d = new Date(iso)
  const now = new Date()
  const sameYear = d.getFullYear() === now.getFullYear()
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) })
}

const dueIso = (days: number) => {
  const d = new Date()
  d.setDate(d.getDate() + days)
  d.setHours(17, 0, 0, 0)
  return d.toISOString()
}

export function StatusPill({ t, ctx, className }: { t: Task; ctx: PillCtx; className?: string }) {
  const label = STATUS_LABEL[t.status] ?? t.status
  if (!ctx.canEdit)
    return (
      <span className={cn('inline-flex items-center gap-1.5 text-[11px] text-muted', className)}>
        <span className="h-2 w-2 rounded-full" style={{ background: STATUS_COLOR[t.status] }} />
        {label}
      </span>
    )
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill dot={STATUS_COLOR[t.status]} active={open} title="Change status">
          {label}
        </FieldPill>
      )}
      items={TASK_STATUSES.map((s) => ({
        label: STATUS_LABEL[s] ?? s,
        checked: t.status === s,
        onSelect: () => ctx.onPatch({ status: s }),
      }))}
    />
  )
}

export function PriorityPill({ t, ctx, className }: { t: Task; ctx: PillCtx; className?: string }) {
  const flag = <Flag size={11} style={{ color: PRIORITY_COLOR[t.priority] }} />
  if (!ctx.canEdit)
    return (
      <span className={cn('inline-flex items-center gap-1.5 text-[11px] text-muted', className)}>
        {flag}
        {t.priority}
      </span>
    )
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill icon={flag} active={open} title="Change priority">
          {t.priority}
        </FieldPill>
      )}
      items={[...PRIORITIES].reverse().map((p) => ({
        label: p,
        icon: <Flag size={12} style={{ color: PRIORITY_COLOR[p] }} />,
        checked: t.priority === p,
        onSelect: () => ctx.onPatch({ priority: p }),
      }))}
    />
  )
}

export function DuePill({ t, ctx, className }: { t: Task; ctx: PillCtx; className?: string }) {
  const late = isOverdueTask(t)
  const label = t.dueDate ? fmtDue(t.dueDate) : 'Due'
  if (!ctx.canEdit) {
    if (!t.dueDate) return null
    return (
      <span className={cn('text-[11px]', late ? 'font-medium text-[color:var(--theme-danger)]' : 'text-muted', className)}>
        {label}
      </span>
    )
  }
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill
          icon={<CalendarDays size={11} />}
          active={open}
          empty={!t.dueDate}
          title="Set due date"
          className={cn(late && 'font-medium !text-[color:var(--theme-danger)]')}
        >
          {label}
        </FieldPill>
      )}
      items={[
        { label: 'Today', onSelect: () => ctx.onPatch({ dueDate: dueIso(0) }) },
        { label: 'Tomorrow', onSelect: () => ctx.onPatch({ dueDate: dueIso(1) }) },
        { label: 'Next week', onSelect: () => ctx.onPatch({ dueDate: dueIso(7) }) },
        ...(t.dueDate ? (['sep', { label: 'Clear', danger: true, onSelect: () => ctx.onPatch({ dueDate: null }) }] as ContextMenuEntry[]) : []),
      ]}
      footer={(close) => (
        <input
          type="date"
          value={t.dueDate ? t.dueDate.slice(0, 10) : ''}
          onChange={(e) => {
            const v = e.target.value
            if (v) {
              ctx.onPatch({ dueDate: new Date(`${v}T17:00`).toISOString() })
              close()
            }
          }}
          className="w-full cursor-pointer bg-transparent text-xs text-fg focus:outline-none"
        />
      )}
    />
  )
}

export function EstimatePill({ t, ctx, className }: { t: Task; ctx: PillCtx; className?: string }) {
  const label = t.estimatedHours != null ? `${t.estimatedHours}h` : 'Estimate'
  if (!ctx.canEdit) {
    if (t.estimatedHours == null) return null
    return <span className={cn('text-[11px] text-muted', className)}>{label}</span>
  }
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill icon={<Timer size={11} />} active={open} empty={t.estimatedHours == null} title="Set estimate (hours)">
          {label}
        </FieldPill>
      )}
      items={[0.5, 1, 2, 4, 8].map((h) => ({
        label: `${h}h`,
        checked: t.estimatedHours === h,
        onSelect: () => ctx.onPatch({ estimatedHours: h }),
      }))}
      footer={(close) => (
        <input
          type="number"
          min={0}
          max={999}
          step={0.5}
          placeholder="Custom hours"
          defaultValue={t.estimatedHours ?? ''}
          onKeyDown={(e) => {
            if (e.key !== 'Enter') return
            const v = (e.target as HTMLInputElement).value.trim()
            const n = v === '' ? null : Number(v)
            if (n === null || (!Number.isNaN(n) && n >= 0)) {
              ctx.onPatch({ estimatedHours: n })
              close()
            }
          }}
          className="w-full bg-transparent text-xs text-fg [appearance:textfield] focus:outline-none"
        />
      )}
    />
  )
}

export function AssigneesPill({ t, ctx, className }: { t: Task; ctx: PillCtx; className?: string }) {
  const infos = t.assignees.map((a) => assigneeInfo(a, ctx.agents, ctx.members))
  const summary =
    infos.length === 0 ? 'Assign' : infos.length === 1 ? infos[0]!.label : `${infos.length} assignees`
  const avatars = (
    <span className="flex items-center gap-1.5">
      {infos.length > 0 && (
        <span className="flex -space-x-1.5">
          {infos.slice(0, 3).map((a) => (
            <Avatar key={a.key} name={a.label} className="h-4.5 w-4.5 ring-2 ring-[color:var(--theme-panel)]" />
          ))}
        </span>
      )}
      <span className="min-w-0 truncate">{summary}</span>
    </span>
  )
  if (!ctx.canEdit) {
    if (infos.length === 0) return null
    return <span className={cn('inline-flex items-center text-[11px] text-muted', className)}>{avatars}</span>
  }
  const toggle = (key: string) =>
    ctx.onPatch({ assignees: t.assignees.includes(key) ? t.assignees.filter((a) => a !== key) : [...t.assignees, key] })
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill
          active={open}
          empty={infos.length === 0}
          icon={infos.length === 0 ? <UserRound size={11} /> : undefined}
          title="Assign teammates or agents"
        >
          {avatars}
        </FieldPill>
      )}
      items={() => [
        ...ctx.members.map((m) => {
          const key = userAssignee(m.userId)
          return {
            label: (m.name ?? m.email ?? 'teammate') + (m.userId === ctx.meId ? ' (me)' : ''),
            checked: t.assignees.includes(key),
            keepOpen: true,
            onSelect: () => toggle(key),
          }
        }),
        ...(ctx.members.length && ctx.agents.length ? (['sep'] as ContextMenuEntry[]) : []),
        ...ctx.agents.map((a) => ({
          label: a.label,
          checked: t.assignees.includes(a.id),
          keepOpen: true,
          onSelect: () => toggle(a.id),
        })),
      ]}
    />
  )
}
