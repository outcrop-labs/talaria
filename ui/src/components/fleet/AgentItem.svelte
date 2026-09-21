<script lang="ts" module>
  /** Everything the row and the tile each used to derive for themselves, handed
   *  to the frame they supply: nothing about geometry lives in here. */
  export interface AgentFrame {
    /** The container is up — drives the lifecycle half of AgentControls and the
     *  Start/Stop half of the agent menu. */
    running: boolean
    /** Opens the manage modal this component owns. */
    openManage: () => void
    /** The frame root's `oncontextmenu` (the manage/duplicate/lifecycle menu). */
    onContextMenu: (e: MouseEvent) => void
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import type { AgentContainers, AgentDef, LlmEndpoint } from '@/lib/fleet-defs'
  import AgentRetireModal from '@/routes/app/AgentRetireModal.svelte'
  import { healthOf, useAgentMenu } from '@/routes/app/agents.svelte'
  import AgentManageModal from './AgentManageModal.svelte'

  // One agent as an ITEM: the health it reads off container reality, the
  // right-click menu, and the manage/retire modal tail. The roster frames it
  // two ways — AgentListRow and AgentTile — which own their geometry and are
  // handed the derivation through `frame`; this is the half they shared.
  let {
    def: d,
    containers,
    endpoints,
    onDuplicate,
    frame,
  }: {
    def: AgentDef
    containers: AgentContainers | null
    endpoints: LlmEndpoint[]
    onDuplicate: () => void
    /** The frame: row or tile markup, rendering whatever it wants of AgentFrame. */
    frame: Snippet<[AgentFrame]>
  } = $props()

  let manage = $state(false)
  const openManage = () => (manage = true)
  const running = $derived(healthOf(d, containers).running)
  const am = useAgentMenu(() => d, () => running, openManage, () => onDuplicate())
</script>

{@render frame({ running, openManage, onContextMenu: am.onContextMenu })}
<ContextMenu menu={am.menu} />
{#if am.retiring}
  <AgentRetireModal def={d} onClose={() => (am.retiring = false)} onConfirm={() => void am.act('retire', 'retiring')} />
{/if}
<!-- isAdmin: both frames showed the roster to managers only, so both passed it
     unconditionally — the modal's read-only mode was never reached here. -->
{#if manage}
  <AgentManageModal open={manage} onClose={() => (manage = false)} def={d} {endpoints} isAdmin />
{/if}