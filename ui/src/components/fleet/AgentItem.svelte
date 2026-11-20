<script lang="ts" module>
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import type { AgentMenu } from '@/routes/app/agents.svelte'

  /** Everything the row and the tile each used to derive for themselves, handed
   *  to the frame they supply: nothing about geometry lives in here. */
  export interface AgentFrame {
    /** The container is up. */
    running: boolean
    /** Opens the manage modal this component owns. */
    openManage: () => void
    /** The frame root's `oncontextmenu` (the manage/duplicate/lifecycle menu). */
    onContextMenu: (e: MouseEvent) => void
    /** The same menu's entries, for the frame's own "…" button. */
    menuItems: () => ContextMenuEntry[]
    /** Runs one control on this agent (the menu's own instance). */
    act: AgentMenu['act']
    /** The label of the control being carried out, or null. */
    pending: string | null
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { rollInProgress, type AgentContainers, type AgentDef, type FleetRollRow, type LlmEndpoint } from '@/lib/fleet-defs'
  import AgentRetireModal from '@/routes/app/AgentRetireModal.svelte'
  import { healthOf, useAgentMenu } from '@/routes/app/agents.svelte'
  import AgentManageModal from './AgentManageModal.svelte'

  // One agent as an ITEM: the health it reads off container reality, its
  // menu, and the manage/retire modal tail. The roster frames it
  // two ways — AgentListRow and AgentTile — which own their geometry and are
  // handed the derivation through `frame`; this is the half they shared.
  let {
    def: d,
    containers,
    endpoints,
    onDuplicate,
    roll,
    frame,
  }: {
    def: AgentDef
    containers: AgentContainers | null
    endpoints: LlmEndpoint[]
    onDuplicate: () => void
    /** This agent's row in the roll picture (admins see it; undefined otherwise). */
    roll?: FleetRollRow
    /** The frame: row or tile markup, rendering whatever it wants of AgentFrame. */
    frame: Snippet<[AgentFrame]>
  } = $props()

  let manage = $state(false)
  const openManage = () => (manage = true)
  const running = $derived(healthOf(d, containers).running)
  const rolling = $derived(rollInProgress(roll))
  const am = useAgentMenu(() => d, () => running, openManage, () => onDuplicate(), () => rolling)
</script>

{@render frame({ running, openManage, onContextMenu: am.onContextMenu, menuItems: am.entries, act: am.act, pending: am.pending })}
<ContextMenu menu={am.menu} />
{#if am.retiring}
  <AgentRetireModal def={d} onClose={() => (am.retiring = false)} onConfirm={() => void am.act('retire', 'retiring')} />
{/if}
<!-- canManage rides on the def: the roster now shows agents somebody holds
     `agents.manage` over but does NOT manage, and those open read-only —
     which is the mode this modal always had and never reached. -->
{#if manage}
  <AgentManageModal open={manage} onClose={() => (manage = false)} def={d} {endpoints} canManage={d.canManage} />
{/if}