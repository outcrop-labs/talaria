// The board filter bar — ClickUp-grade facets, Mercury-clean. Each facet is a
// FieldPill opening a keep-open multi-select; the pill itself reads as the
// active filter ("Status · 2"). Values are ORed within a facet, ANDed across
// facets; everything lives in the URL (the route owns encoding).
import { CalendarDays, Flag, Tag, UserRound, X } from 'lucide-react'
import { DropdownMenu, type ContextMenuEntry } from '@/components/ui/context-menu'
import { FieldPill } from '@/components/ui/field-pill'
import { userAssignee } from '@/lib/assignees'
import type { BoardMember } from '@/lib/boards'
import { PRIORITIES, PRIORITY_COLOR, TASK_STATUSES } from '@/lib/task-const'
import type { BoardLabel } from '@/lib/boards'
import { LABEL_CSS, STATUS_COLOR } from './field-pills'
import { statusColorOf, statusLabelOf, type BoardStatus } from '@/lib/statuses'

export interface BoardFilters {
  statuses: string[]
  assignees: string[]
  priorities: string[]
  labels: string[]
  due: '' | 'overdue' | 'today' | 'week' | 'none'
}

export const EMPTY_FILTERS: BoardFilters = { statuses: [], assignees: [], priorities: [], labels: [], due: '' }

export const filtersActive = (f: BoardFilters): boolean =>
  f.statuses.length > 0 || f.assignees.length > 0 || f.priorities.length > 0 || f.labels.length > 0 || f.due !== ''

const DUE_LABEL: Record<Exclude<BoardFilters['due'], ''>, string> = {
  overdue: 'Overdue',
  today: 'Due today',
  week: 'Due this week',
  none: 'No due date',
}

function FacetPill({
  icon,
  label,
  count,
  items,
}: {
  icon: React.ReactNode
  label: string
  count: number
  items: ContextMenuEntry[] | (() => ContextMenuEntry[])
}) {
  return (
    <DropdownMenu
      align="left"
      trigger={(open) => (
        <FieldPill icon={icon} active={open || count > 0} className="h-9 rounded-md px-2">
          {label}
          {count > 0 ? ` · ${count}` : ''}
        </FieldPill>
      )}
      items={items}
    />
  )
}

export function FilterBar({
  value,
  onChange,
  members,
  agents,
  labels,
  statuses,
  meId,
}: {
  value: BoardFilters
  onChange: (next: BoardFilters) => void
  members: BoardMember[]
  agents: Array<{ id: string; label: string }>
  labels: BoardLabel[]
  statuses?: BoardStatus[]
  meId?: string | null
}) {
  const toggle = (facet: 'statuses' | 'assignees' | 'priorities' | 'labels', v: string) => {
    const cur = value[facet]
    onChange({ ...value, [facet]: cur.includes(v) ? cur.filter((x) => x !== v) : [...cur, v] })
  }

  return (
    <div className="flex flex-wrap items-center gap-1">
      <FacetPill
        icon={<span className="h-1.5 w-1.5 rounded-full bg-accent" />}
        label="Status"
        count={value.statuses.length}
        items={[...(statuses?.length ? statuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])), 'failed', 'cancelled'].map((s) => ({
          label: statusLabelOf(s, statuses ?? []),
          icon: (
            <span
              className="h-1.5 w-1.5 rounded-full"
              style={{ background: statuses?.length ? statusColorOf(s, statuses) : STATUS_COLOR[s] ?? 'var(--theme-muted)' }}
            />
          ),
          checked: value.statuses.includes(s),
          keepOpen: true,
          onSelect: () => toggle('statuses', s),
        }))}
      />
      <FacetPill
        icon={<UserRound size={12} />}
        label="Assignee"
        count={value.assignees.length}
        items={() => [
          ...(meId
            ? [
                {
                  label: 'Me',
                  checked: value.assignees.includes(userAssignee(meId)),
                  keepOpen: true,
                  onSelect: () => toggle('assignees', userAssignee(meId)),
                },
              ]
            : []),
          {
            label: 'Unassigned',
            checked: value.assignees.includes('__none'),
            keepOpen: true,
            onSelect: () => toggle('assignees', '__none'),
          },
          'sep' as const,
          ...members
            .filter((m) => m.userId !== meId)
            .map((m) => ({
              label: m.name ?? m.email ?? 'teammate',
              checked: value.assignees.includes(userAssignee(m.userId)),
              keepOpen: true,
              onSelect: () => toggle('assignees', userAssignee(m.userId)),
            })),
          ...(agents.length ? (['sep'] as ContextMenuEntry[]) : []),
          ...agents.map((a) => ({
            label: a.label,
            checked: value.assignees.includes(a.id),
            keepOpen: true,
            onSelect: () => toggle('assignees', a.id),
          })),
        ]}
      />
      <FacetPill
        icon={<Flag size={12} />}
        label="Priority"
        count={value.priorities.length}
        items={[...PRIORITIES].reverse().map((p) => ({
          label: p,
          icon: <Flag size={12} style={{ color: PRIORITY_COLOR[p] }} />,
          checked: value.priorities.includes(p),
          keepOpen: true,
          onSelect: () => toggle('priorities', p),
        }))}
      />
      {labels.length > 0 && (
        <FacetPill
          icon={<Tag size={12} />}
          label="Label"
          count={value.labels.length}
          items={labels.map((l) => ({
            label: l.name,
            icon: <span className="h-1.5 w-1.5 rounded-full" style={{ background: LABEL_CSS[l.color] }} />,
            checked: value.labels.includes(l.name),
            keepOpen: true,
            onSelect: () => toggle('labels', l.name),
          }))}
        />
      )}
      <FacetPill
        icon={<CalendarDays size={12} />}
        label={value.due ? DUE_LABEL[value.due] : 'Due'}
        count={value.due ? 1 : 0}
        items={(Object.keys(DUE_LABEL) as Array<Exclude<BoardFilters['due'], ''>>).map((d) => ({
          label: DUE_LABEL[d],
          checked: value.due === d,
          onSelect: () => onChange({ ...value, due: value.due === d ? '' : d }),
        }))}
      />
      {filtersActive(value) && (
        <button
          onClick={() => onChange(EMPTY_FILTERS)}
          title="Clear all filters"
          className="flex h-9 items-center gap-1 rounded-md px-2 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger"
        >
          <X size={12} /> Clear
        </button>
      )}
    </div>
  )
}
