// Shared pieces of the Agents roster (Agents.svelte + its tiles/rows): the
// health mapping, the per-agent control logic, and the right-click menu.
import { useQueryClient } from '@tanstack/svelte-query'
import { Archive, Copy, Play, Repeat, RotateCw, SlidersHorizontal, Square, Trash2, UserPlus } from '@lucide/svelte'
import { confirm } from '@/components/ui/confirm.svelte'
import { useContextMenu, type ContextMenuController, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
import { useSession } from '@/lib/session'
import { controlAgent, type AgentContainers, type AgentDef, type FleetAction } from '@/lib/fleet-defs'

// ── Health: one word from container reality. 'warming' = the healthcheck's
// start_period — the container is up but its gateway isn't serving yet.
// 'draft' = hired without a container ON PURPOSE (TALA-11): the hire run
// wrote the def, rendered the config, and skipped the boot because the person
// asked to commit the hire later. Nothing is wrong with it, which is why it
// is not 'down' — a red dot on a deliberate state is a lie the roster used
// to tell. Managed+created only: an imported def without a container was
// never Talaria's to boot, so it stays 'down' where it belongs. ──
export type Health = 'up' | 'warming' | 'degraded' | 'down' | 'draft' | 'retired'
export const HEALTH_COLOR: Record<Health, string> = {
  up: 'var(--theme-success)',
  warming: 'var(--theme-warning)',
  degraded: 'var(--theme-warning)',
  down: 'var(--theme-danger)',
  draft: 'var(--theme-ink-dim)',
  retired: 'var(--theme-line)',
}

export function healthOf(d: AgentDef, c: AgentContainers | null): { health: Health; running: boolean } {
  if (!d.enabled) return { health: 'retired', running: false }
  // DRAFT: enabled + managed + created, and the containers read carries no
  // container state at all — the hire's start=false path. Checked before the
  // running read so a draft that is somehow up (someone started it) still
  // reads as up: a live container is a stronger fact than the hire's intent.
  if (d.managed && d.source === 'created' && (c === null || c.managed === null)) {
    return { health: 'draft', running: false }
  }
  const state = c?.managed ?? null
  const running = state?.state === 'running'
  if (!running) return { health: 'down', running: false }
  if (state?.health === 'starting') return { health: 'warming', running: true }
  return { health: state?.health === 'unhealthy' ? 'degraded' : 'up', running: true }
}

// Confirm copy shared by the Controls buttons and the right-click menu, so the
// two paths can never drift apart.
export const RESTART_CONFIRM = 'Restart this agent now? Any reply it is mid-way through will be dropped.'
export const deleteForeverConfirm = (d: AgentDef) =>
  `Permanently delete ${d.displayName}? This removes its definition, version history, secrets, and (for Talaria-created agents) its memory volume. Chats, tickets, and ledger history it produced are kept. This cannot be undone.`

export interface AgentControlsState {
  readonly pending: string | null
  act: (action: FleetAction, label: string, confirmMsg?: string) => Promise<void>
}

// Shared control logic for one agent def. Takes a getter so the def stays
// live across roster refetches (call during component init).
export function useAgentControls(def: () => AgentDef): AgentControlsState {
  const qc = useQueryClient()
  let pending = $state<string | null>(null)
  const act = async (action: FleetAction, label: string, confirmMsg?: string) => {
    if (confirmMsg && !(await confirm({ title: label, message: confirmMsg, confirmLabel: label }))) return
    pending = label
    try {
      await controlAgent(def().id, action)
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } finally {
      pending = null
    }
  }
  return {
    get pending() {
      return pending
    },
    act,
  }
}

export interface AgentMenu {
  menu: ContextMenuController
  onContextMenu: (e: MouseEvent) => void
  act: (action: FleetAction, label: string, confirmMsg?: string) => Promise<void>
  /** True while the retire double opt-in modal should show. */
  retiring: boolean
}

// Right-click menu for a tile / list row — the same actions as the Controls
// cluster (same admin guards, same confirms), reachable from anywhere on the
// row. Its own useAgentControls instance drives the acts; AgentRetireModal
// keeps its double opt-in. The caller renders `<ContextMenu menu={x.menu} />`
// plus the retire modal off `x.retiring` (this was the `overlays` JSX the
// React hook returned).
export function useAgentMenu(
  def: () => AgentDef,
  running: () => boolean,
  onManage: () => void,
  onDuplicate: () => void,
): AgentMenu {
  const session = useSession()
  const controls = useAgentControls(def)
  const menu = useContextMenu()
  let retiring = $state(false)

  const onContextMenu = (e: MouseEvent) => {
    const d = def()
    const isAdmin = session.data?.role === 'admin'
    const act = controls.act
    if (!d.enabled) {
      // Retired: re-hire, duplicate as a template, delete forever (admin).
      const entries: ContextMenuEntry[] = [
        { label: 'Duplicate to a new agent', icon: [Copy, { size: 14 }], onSelect: onDuplicate },
        { label: 'Re-hire', icon: [UserPlus, { size: 14 }], onSelect: () => void act('unretire', 're-hiring') },
        ...(isAdmin
          ? ([
              'sep',
              {
                label: 'Delete forever',
                icon: [Trash2, { size: 14 }],
                danger: true,
                onSelect: () => void act('delete', 'Delete forever', deleteForeverConfirm(d)),
              },
            ] as ContextMenuEntry[])
          : []),
      ]
      menu.openMenu(e, entries)
      return
    }
    const entries: ContextMenuEntry[] = [
      { label: 'Manage', icon: [SlidersHorizontal, { size: 14 }], onSelect: onManage },
      { label: 'Duplicate to a new agent', icon: [Copy, { size: 14 }], onSelect: onDuplicate },
      'sep',
      ...(running()
        ? ([
            { label: 'Stop', icon: [Square, { size: 13, fill: 'currentColor' }], onSelect: () => void act('stop', 'stopping') },
            { label: 'Restart', icon: [RotateCw, { size: 13 }], onSelect: () => void act('restart', 'restarting', RESTART_CONFIRM) },
            ...(isAdmin ? ([{ label: 'Roll', icon: [Repeat, { size: 13 }], onSelect: () => void act('roll', 'rolling') }] as ContextMenuEntry[]) : []),
          ] as ContextMenuEntry[])
        : ([
            { label: 'Start', icon: [Play, { size: 13, fill: 'currentColor' }], onSelect: () => void act('up', 'starting') },
          ] as ContextMenuEntry[])),
      'sep',
      { label: 'Retire', icon: [Archive, { size: 14 }], danger: true, onSelect: () => (retiring = true) },
    ]
    menu.openMenu(e, entries)
  }

  return {
    menu,
    onContextMenu,
    act: controls.act,
    get retiring() {
      return retiring
    },
    set retiring(v: boolean) {
      retiring = v
    },
  }
}
