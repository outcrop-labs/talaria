// Row menus for plans and research runs. The same entries the record page
// offers — a context menu is a shortcut, never the only home of an action.
import { copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
import type { Conversation } from '@/lib/conversations.svelte'

export function planRowMenu(
  c: Conversation,
  opts: {
    path: string
    open: () => void
    archived: boolean
    onRename: () => void
    onArchive: () => void
    onRestore: () => void
    onDelete: () => void
  },
): ContextMenuEntry[] {
  const items: ContextMenuEntry[] = [
    { label: 'Open', onSelect: opts.open },
    { label: 'Copy link', onSelect: () => copyAppLink(opts.path) },
    { label: 'Rename', onSelect: opts.onRename },
  ]
  // Archive and delete hide the plan from every member. Only the owner.
  if (c.role === 'owner') {
    items.push(
      'sep',
      opts.archived
        ? { label: 'Restore', onSelect: opts.onRestore }
        : { label: 'Archive', danger: true, onSelect: opts.onArchive },
      { label: 'Delete', danger: true, onSelect: opts.onDelete },
    )
  }
  return items
}

/** Rename and remove, for a run the caller may manage. No archive: a run is
 *  ephemeral, and delete is already its stop. */
export function researchRowMenu(
  id: string,
  open: () => void,
  canManage: boolean,
  actions: { rename: () => void; remove: () => void },
): ContextMenuEntry[] {
  const items: ContextMenuEntry[] = [
    { label: 'Open', onSelect: open },
    { label: 'Copy link', onSelect: () => copyAppLink(`/research/${id}`) },
  ]
  if (canManage) {
    items.push({ label: 'Rename', onSelect: actions.rename }, 'sep', {
      label: 'Remove',
      danger: true,
      onSelect: actions.remove,
    })
  }
  return items
}
