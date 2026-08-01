// Ticket property pills — the ClickUp-style direct-manipulation layer, done
// the Mercury way. Each pill shows the value quietly, lights up on hover
// (FieldPill), and opens its picker in place (DropdownMenu). One set of
// components serves kanban cards, list rows, and group headers, so every
// surface manipulates tickets identically. All pickers stop propagation —
// the row/card click still opens the ticket.
import { useQueryClient } from '@tanstack/react-query'
import { CalendarDays, Flag, Palette, Tag, Timer, UserRound } from 'lucide-react'
import { Avatar } from '@/components/ui/avatar'
import { DropdownMenu, type ContextMenuEntry } from '@/components/ui/context-menu'
import { FieldPill } from '@/components/ui/field-pill'
import { cn } from '@/lib/cn'
import { assigneeInfo, userAssignee } from '@/lib/assignees'
import { createBoardLabel, type BoardLabel, type BoardMember, type LabelColor } from '@/lib/boards'
import { statusColorOf, statusLabelOf, type BoardStatus } from '@/lib/statuses'
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
  tags?: string[]
}

/** The label palette, keyed by the stored color name. Semantic hues ride the
 *  theme tokens (they follow light/dark); orange rides chart-2. The remaining
 *  hues are DATA colors (user-picked label identities, like chart series)
 *  with no token equivalent — kept literal by design. Blue stays literal:
 *  chart-1 (#68B6C8) would collide with the adjacent cyan/teal entries. */
export const LABEL_CSS: Record<LabelColor, string> = {
  slate: 'var(--theme-muted)',
  bronze: 'var(--theme-accent)',
  green: 'var(--theme-success)',
  amber: 'var(--theme-warning)',
  red: 'var(--theme-danger)',
  blue: '#6b9bd1',
  orange: 'var(--theme-chart-2)',
  purple: '#a78bda',
  teal: '#5fb8ad',
  pink: '#d189a8',
  lime: '#a2c05b',
  cyan: '#5fb6d4',
  indigo: '#7a86d9',
  magenta: '#c069c9',
  olive: '#a8a45e',
  brown: '#a8795a',
}

export const labelColor = (name: string, labels: BoardLabel[]): string =>
  LABEL_CSS[labels.find((l) => l.name === name)?.color ?? 'slate']

/** The one tinted label chip — used on cards, rows, facets, and settings.
 *  Mono chip voice (spec §8). */
export function LabelChip({ name, labels, className }: { name: string; labels: BoardLabel[]; className?: string }) {
  const c = labelColor(name, labels)
  return (
    <span
      className={cn('max-w-28 truncate rounded-full border px-1.5 py-0.5 font-mono text-[10px] normal-case tracking-[0.05em]', className)}
      style={{
        color: c,
        borderColor: `color-mix(in srgb, ${c} 45%, transparent)`,
        background: `color-mix(in srgb, ${c} 10%, transparent)`,
      }}
    >
      {name}
    </span>
  )
}

export interface PillCtx {
  canEdit: boolean
  onPatch: (p: TicketPatch) => void
  agents: Array<{ id: string; label: string }>
  members: BoardMember[]
  meId?: string | null
  /** The board's label registry — powers LabelsPill + tinted chips. */
  labels?: BoardLabel[]
  /** The board's status set — powers StatusPill options + closed checks. */
  statuses?: BoardStatus[]
  boardId?: string
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

/** Closed = done-category on this board (or the legacy terminal keys). */
export const isClosedStatus = (key: string, statuses?: BoardStatus[]): boolean =>
  statuses?.find((s) => s.key === key)?.category === 'done' || ['done', 'cancelled', 'failed'].includes(key)

export const isOverdueTask = (t: Pick<Task, 'dueDate' | 'status'>, statuses?: BoardStatus[]) =>
  !!t.dueDate && new Date(t.dueDate).getTime() < Date.now() && !isClosedStatus(t.status, statuses)

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
  const sts = ctx.statuses ?? []
  const label = statusLabelOf(t.status, sts)
  const dot = sts.length ? statusColorOf(t.status, sts) : STATUS_COLOR[t.status]
  if (!ctx.canEdit)
    return (
      <span className={cn('inline-flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>
        <span className="h-1.5 w-1.5 rounded-full" style={{ background: dot }} />
        {label}
      </span>
    )
  const options = sts.length
    ? sts.map((st) => ({ key: st.key, label: st.label, color: statusColorOf(st.key, sts) }))
    : TASK_STATUSES.map((k) => ({ key: k as string, label: STATUS_LABEL[k] ?? k, color: STATUS_COLOR[k] ?? 'var(--theme-muted)' }))
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill dot={dot} active={open} title="Change status">
          {label}
        </FieldPill>
      )}
      items={options.map((o) => ({
        label: o.label,
        icon: <span className="h-1.5 w-1.5 rounded-full" style={{ background: o.color }} />,
        checked: t.status === o.key,
        onSelect: () => ctx.onPatch({ status: o.key as Task['status'] }),
      }))}
    />
  )
}

export function PriorityPill({ t, ctx, className }: { t: Task; ctx: PillCtx; className?: string }) {
  const flag = <Flag size={11} style={{ color: PRIORITY_COLOR[t.priority] }} />
  if (!ctx.canEdit)
    return (
      <span className={cn('inline-flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>
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

export function DuePill({ t, ctx, className, ghost, persistent }: { t: Task; ctx: PillCtx; className?: string; ghost?: boolean; persistent?: boolean }) {
  const late = isOverdueTask(t, ctx.statuses)
  const label = t.dueDate ? fmtDue(t.dueDate) : 'Due'
  if (!ctx.canEdit) {
    if (!t.dueDate) return null
    return (
      <span className={cn('font-mono text-[10px] uppercase tracking-[0.05em]', late ? 'font-medium text-danger' : 'text-muted', className)}>
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
          persistent={persistent}
          icon={<CalendarDays size={11} />}
          active={open}
          empty={!t.dueDate}
          title="Set due date"
          className={cn(
            late && 'font-medium !text-danger',
            // Ghost: an unset property stays invisible until the card is
            // hovered (or its picker is open) — quiet cards, one-click set.
            ghost && !persistent && !t.dueDate && !open && 'opacity-0 transition-opacity group-hover:opacity-100',
          )}
        >
          {ghost && !persistent && !t.dueDate ? '' : label}
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

export function EstimatePill({ t, ctx, className, ghost, persistent }: { t: Task; ctx: PillCtx; className?: string; ghost?: boolean; persistent?: boolean }) {
  const label = t.estimatedHours != null ? `${t.estimatedHours}h` : 'Estimate'
  if (!ctx.canEdit) {
    if (t.estimatedHours == null) return null
    return <span className={cn('font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>{label}</span>
  }
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill
          persistent={persistent}
          icon={<Timer size={11} />}
          active={open}
          empty={t.estimatedHours == null}
          title="Set estimate (hours)"
          className={cn(ghost && !persistent && t.estimatedHours == null && !open && 'opacity-0 transition-opacity group-hover:opacity-100')}
        >
          {ghost && !persistent && t.estimatedHours == null ? '' : label}
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

export function AssigneesPill({ t, ctx, className, ghost, persistent }: { t: Task; ctx: PillCtx; className?: string; ghost?: boolean; persistent?: boolean }) {
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
    return <span className={cn('inline-flex items-center font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>{avatars}</span>
  }
  const toggle = (key: string) =>
    ctx.onPatch({ assignees: t.assignees.includes(key) ? t.assignees.filter((a) => a !== key) : [...t.assignees, key] })
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill
          persistent={persistent}
          active={open}
          empty={infos.length === 0}
          icon={infos.length === 0 ? <UserRound size={11} /> : undefined}
          title="Assign teammates or agents"
          className={cn(ghost && !persistent && infos.length === 0 && !open && 'opacity-0 transition-opacity group-hover:opacity-100')}
        >
          {ghost && !persistent && infos.length === 0 ? '' : avatars}
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

export function LabelsPill({ t, ctx, className, ghost, persistent }: { t: Task; ctx: PillCtx; className?: string; ghost?: boolean; persistent?: boolean }) {
  const qc = useQueryClient()
  const labels = ctx.labels ?? []
  const shown = t.tags.slice(0, 2)
  if (!ctx.canEdit) {
    if (t.tags.length === 0) return null
    return (
      <span className={cn('inline-flex items-center gap-1', className)}>
        {shown.map((n) => (
          <LabelChip key={n} name={n} labels={labels} />
        ))}
        {t.tags.length > 2 && <span className="text-[10px] text-muted">+{t.tags.length - 2}</span>}
      </span>
    )
  }
  const toggle = (name: string) =>
    ctx.onPatch({ tags: t.tags.includes(name) ? t.tags.filter((x) => x !== name) : [...t.tags, name] })
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill
          persistent={persistent}
          active={open}
          empty={t.tags.length === 0}
          icon={t.tags.length === 0 ? <Tag size={11} /> : undefined}
          title="Labels"
          className={cn(ghost && !persistent && t.tags.length === 0 && !open && 'opacity-0 transition-opacity group-hover:opacity-100')}
        >
          {t.tags.length === 0 ? (
            ghost && !persistent ? '' : 'Label'
          ) : (
            <span className="flex items-center gap-1">
              {shown.map((n) => (
                <LabelChip key={n} name={n} labels={labels} />
              ))}
              {t.tags.length > 2 && <span className="text-[10px] text-muted">+{t.tags.length - 2}</span>}
            </span>
          )}
        </FieldPill>
      )}
      items={() =>
        labels.map((l) => ({
          label: l.name,
          icon: <span className="h-1.5 w-1.5 rounded-full" style={{ background: LABEL_CSS[l.color] }} />,
          checked: t.tags.includes(l.name),
          keepOpen: true,
          onSelect: () => toggle(l.name),
        }))
      }
      footer={(close) => (
        <input
          placeholder="New label — Enter creates"
          onKeyDown={(e) => {
            if (e.key !== 'Enter') return
            const name = (e.target as HTMLInputElement).value.trim()
            if (!name) return
            void (async () => {
              if (ctx.boardId) {
                await createBoardLabel(ctx.boardId, name)
                void qc.invalidateQueries({ queryKey: ['board-labels', ctx.boardId] })
              }
              ctx.onPatch({ tags: t.tags.includes(name) ? t.tags : [...t.tags, name] })
              close()
            })()
          }}
          className="w-full bg-transparent text-xs text-fg placeholder:text-muted focus:outline-none"
        />
      )}
    />
  )
}

/** Color picker as a dropdown swatch GRID — same pill grammar as every other
 *  property. Standalone (value/onChange) so the detail rail and any future
 *  surface can use it without a full PillCtx. */
export function ColorPill({
  value,
  onChange,
  disabled,
  className,
}: {
  value: string | null
  onChange: (c: string | null) => void
  disabled?: boolean
  className?: string
}) {
  const colors = Object.keys(LABEL_CSS) as Array<keyof typeof LABEL_CSS>
  if (disabled) {
    if (!value) return null
    return (
      <span className={cn('inline-flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>
        <span className="h-2.5 w-2.5 rounded-full" style={{ background: LABEL_CSS[value as keyof typeof LABEL_CSS] }} />
        {value}
      </span>
    )
  }
  return (
    <DropdownMenu
      align="left"
      className={className}
      trigger={(open) => (
        <FieldPill
          active={open}
          empty={!value}
          dot={value ? LABEL_CSS[value as keyof typeof LABEL_CSS] : undefined}
          icon={value ? undefined : <Palette size={11} />}
          title="Color-code this ticket"
        >
          {value ? '' : 'Color'}
        </FieldPill>
      )}
      items={[]}
      content={(close) => (
        <div className="p-1">
          <div className="grid grid-cols-4 gap-2 p-1">
            {colors.map((c) => (
              <button
                key={c}
                title={c}
                onClick={() => {
                  onChange(value === c ? null : c)
                  close()
                }}
                className={cn(
                  'grid h-6 w-6 place-items-center rounded-full transition-all',
                  value === c ? 'ring-1 ring-[color:var(--theme-fg)]/60 ring-offset-2 ring-offset-[color:var(--theme-panel)]' : 'hover:ring-1 hover:ring-line',
                )}
                style={{ background: LABEL_CSS[c] }}
              />
            ))}
          </div>
          {value && (
            <button
              onClick={() => {
                onChange(null)
                close()
              }}
              className="mt-1 w-full rounded-md px-2 py-1 text-left text-xs text-muted transition-colors hover:bg-hover hover:text-danger"
            >
              Clear color
            </button>
          )}
        </div>
      )}
    />
  )
}
