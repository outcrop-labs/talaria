// Shared pieces of the Agents roster (Agents.svelte + its tiles/rows): the
// health mapping, the per-agent control logic, and the right-click menu.
import { useQueryClient } from '@tanstack/svelte-query'
import { Archive, Copy, Play, Repeat, RotateCw, SlidersHorizontal, Square, Trash2, UserPlus } from '@lucide/svelte'
import { confirm } from '@/components/ui/confirm.svelte'
import { useContextMenu, type ContextMenuController, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
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

// ── A control the route is still carrying out ──
// The control route runs a verb on a task of its own and answers within
// 210 s; past that it answers 202 `{ pending: true }` and the verb goes on (a
// stop waiting out an image pull, a legacy agent's move home, a slow disk).
// The roster used to show nothing then: the call returned, the spinner went,
// and the agent sat in its old state with no word of the verb still under
// way. So a pending answer leaves a note for the agent until the roster
// shows the state the verb was for. Kept per agent id rather than per
// component: the tile's controls and its right-click menu are separate
// instances, and either may have asked.
export interface StillWorking {
  action: FleetAction
  /** Epoch ms of the 202. */
  since: number
  /** A restart's agent seen down or warming since — its bounce, so the next
   *  running reading is the restarted container, not the one before it. */
  bounced: boolean
}

/** A verb's own deadline (the image pull's is 20 minutes) — past this the
 *  note stops claiming anything and the roster's own state stands. */
const STILL_WORKING_FOR_MS = 20 * 60_000

const stillWorkingById = $state<Record<string, StillWorking>>({})

/** The pending verb noted for an agent, if any. */
export function stillWorkingOf(id: string): StillWorking | null {
  return stillWorkingById[id] ?? null
}

/** The note's words: what the verb is still doing. */
export const STILL_WORKING_LABEL: Record<FleetAction, string> = {
  up: 'still starting',
  stop: 'still stopping',
  restart: 'still restarting',
  roll: 'still rolling',
  retire: 'still retiring',
  unretire: 'still re-hiring',
  delete: 'still deleting',
}

/** Whether the roster now shows what a pending verb was for. */
export function stillWorkingArrived(w: StillWorking, d: AgentDef, c: AgentContainers | null, now = Date.now()): boolean {
  if (now - w.since > STILL_WORKING_FOR_MS) return true
  const { running } = healthOf(d, c)
  switch (w.action) {
    case 'up':
    case 'unretire':
      return d.enabled && running
    case 'stop':
      return !running
    case 'retire':
      return !d.enabled && !(c?.managed?.state === 'running')
    case 'restart':
      return w.bounced && running
    default:
      return true
  }
}

/** Note a verb the route answered 202 for: `state` is the container as the
 *  202 described it (a restart already caught down has bounced). */
export function noteStillWorking(id: string, action: FleetAction, state?: string, now = Date.now()): void {
  stillWorkingById[id] = { action, since: now, bounced: state !== undefined && state !== 'running' }
}

/** Fold the roster's latest reading into an agent's note: a restart's bounce
 *  is remembered, and a note whose state has arrived is cleared. */
export function settleStillWorking(d: AgentDef, c: AgentContainers | null): void {
  const w = stillWorkingById[d.id]
  if (!w) return
  if (w.action === 'restart' && !w.bounced) {
    const h = healthOf(d, c).health
    if (h === 'down' || h === 'warming') w.bounced = true
  }
  if (stillWorkingArrived(w, d, c)) delete stillWorkingById[d.id]
}

// Shared control logic for one agent def. Takes a getter so the def stays
// live across roster refetches (call during component init).
export function useAgentControls(def: () => AgentDef): AgentControlsState {
  const qc = useQueryClient()
  let pending = $state<string | null>(null)
  const act = async (action: FleetAction, label: string, confirmMsg?: string) => {
    if (confirmMsg && !(await confirm({ title: label, message: confirmMsg, confirmLabel: label }))) return
    pending = label
    const id = def().id
    delete stillWorkingById[id]
    try {
      const answer = await controlAgent(id, action)
      // 202: the verb outlived the route's answer and goes on. Say so until
      // the roster catches up (AgentControls), rather than nothing.
      if (answer.pending) noteStillWorking(id, action, answer.state)
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      // A roll lands in the fleet-wide roll picture too (the registry the
      // control route and "roll all" share): ask for it now rather than on
      // the next slow poll, so this agent's chip flips at once.
      if (action === 'roll') await qc.invalidateQueries({ queryKey: ['fleet-roll'] })
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
// cluster (same manager guards, same confirms), reachable from anywhere on
// the row. Its own useAgentControls instance drives the acts; AgentRetireModal
// keeps its double opt-in. The caller renders `<ContextMenu menu={x.menu} />`
// plus the retire modal off `x.retiring` (this was the `overlays` JSX the
// React hook returned).
export function useAgentMenu(
  def: () => AgentDef,
  running: () => boolean,
  onManage: () => void,
  onDuplicate: () => void,
  /** The department is mid-roll (its own, or the fleet-wide batch): Roll is
   *  offered but disabled, the same answer the control route gives (409). */
  rolling: () => boolean = () => false,
): AgentMenu {
  const controls = useAgentControls(def)
  const menu = useContextMenu()
  let retiring = $state(false)

  const onContextMenu = (e: MouseEvent) => {
    const d = def()
    // Changing an agent belongs to its managers — the same answer the write
    // routes give (docs/PERMISSIONS.md, "Agent managers"). Duplicate and
    // Manage stay open: one writes a NEW agent, the other opens read-only.
    const canManage = d.canManage
    const act = controls.act
    if (!d.enabled) {
      // Retired: duplicate as a template for anyone; re-hire and delete
      // forever for the people who manage it.
      const entries: ContextMenuEntry[] = [
        { label: 'Duplicate to a new agent', icon: [Copy, { size: 14 }], onSelect: onDuplicate },
        ...(canManage
          ? ([
              { label: 'Re-hire', icon: [UserPlus, { size: 14 }], onSelect: () => void act('unretire', 're-hiring') },
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
      { label: canManage ? 'Manage' : 'View', icon: [SlidersHorizontal, { size: 14 }], onSelect: onManage },
      { label: 'Duplicate to a new agent', icon: [Copy, { size: 14 }], onSelect: onDuplicate },
      ...(canManage
        ? ([
            'sep',
            ...(running()
              ? ([
                  { label: 'Stop', icon: [Square, { size: 13, fill: 'currentColor' }], onSelect: () => void act('stop', 'stopping') },
                  { label: 'Restart', icon: [RotateCw, { size: 13 }], onSelect: () => void act('restart', 'restarting', RESTART_CONFIRM) },
                  {
                    label: rolling() ? 'Rolling now' : 'Roll',
                    icon: [Repeat, { size: 13 }],
                    disabled: rolling(),
                    onSelect: () => void act('roll', 'rolling'),
                  },
                ] as ContextMenuEntry[])
              : ([
                  { label: 'Start', icon: [Play, { size: 13, fill: 'currentColor' }], onSelect: () => void act('up', 'starting') },
                ] as ContextMenuEntry[])),
            'sep',
            { label: 'Retire', icon: [Archive, { size: 14 }], danger: true, onSelect: () => (retiring = true) },
          ] as ContextMenuEntry[])
        : []),
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
