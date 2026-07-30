// The one right-click menu for a ticket — kanban cards and list rows serve
// the SAME entries: open/copy shortcuts, then quick controls (move, priority,
// due presets, assign-to-me), then archive. Callers own the actual mutations.
import { Archive, ArrowRight, CalendarDays, ExternalLink, Flag, Hash, Link as LinkIcon, UserRound } from 'lucide-react'
import { copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu'
import { userAssignee } from '@/lib/assignees'
import { PRIORITIES, STATUS_LABEL, TASK_STATUSES, type Task, type TaskStatus, type Priority } from '@/lib/task-const'

export interface TicketMenuOpts {
  canEdit: boolean
  meId?: string | null
  onOpen: () => void
  onPatch: (p: { status?: TaskStatus; priority?: Priority; dueDate?: string | null; assignees?: string[] }) => void
  onArchive: () => void
}

const dueIso = (days: number) => {
  const d = new Date()
  d.setDate(d.getDate() + days)
  d.setHours(17, 0, 0, 0)
  return d.toISOString()
}

export function ticketMenuEntries(t: Task, o: TicketMenuOpts): ContextMenuEntry[] {
  const items: ContextMenuEntry[] = [
    { label: 'Open', icon: <ExternalLink size={14} />, onSelect: o.onOpen },
    { label: 'Copy link', icon: <LinkIcon size={14} />, onSelect: () => copyAppLink(`/boards/${t.boardId}/${t.id}`) },
  ]
  if (t.ticketRef) {
    const ref = t.ticketRef
    items.push({ label: 'Copy ticket ref', icon: <Hash size={14} />, onSelect: () => void navigator.clipboard.writeText(ref) })
  }
  if (o.canEdit) {
    items.push(
      'sep',
      {
        label: 'Move to',
        icon: <ArrowRight size={14} />,
        children: TASK_STATUSES.map((s) => ({
          label: STATUS_LABEL[s] ?? s,
          checked: t.status === s,
          onSelect: () => o.onPatch({ status: s }),
        })),
      },
      {
        label: 'Priority',
        icon: <Flag size={14} />,
        children: [...PRIORITIES].reverse().map((p) => ({
          label: p,
          checked: t.priority === p,
          onSelect: () => o.onPatch({ priority: p }),
        })),
      },
      {
        label: 'Due',
        icon: <CalendarDays size={14} />,
        children: [
          { label: 'Today', onSelect: () => o.onPatch({ dueDate: dueIso(0) }) },
          { label: 'Tomorrow', onSelect: () => o.onPatch({ dueDate: dueIso(1) }) },
          { label: 'Next week', onSelect: () => o.onPatch({ dueDate: dueIso(7) }) },
          ...(t.dueDate
            ? (['sep', { label: 'Clear due date', danger: true, onSelect: () => o.onPatch({ dueDate: null }) }] as ContextMenuEntry[])
            : []),
        ],
      },
    )
    if (o.meId) {
      const meEntry = userAssignee(o.meId)
      const mine = t.assignees.includes(meEntry)
      items.push({
        label: mine ? 'Unassign me' : 'Assign to me',
        icon: <UserRound size={14} />,
        onSelect: () => o.onPatch({ assignees: mine ? t.assignees.filter((a) => a !== meEntry) : [...t.assignees, meEntry] }),
      })
    }
    items.push('sep', {
      label: t.archivedAt ? 'Unarchive' : 'Archive',
      icon: <Archive size={14} />,
      danger: !t.archivedAt,
      onSelect: o.onArchive,
    })
  }
  return items
}
