<script lang="ts">
  import Modal from '@/components/ui/Modal.svelte'
  import { cn } from '@/lib/cn'
  import type { Board } from '@/lib/boards.svelte'
  import GeneralTab from './GeneralTab.svelte'
  import StatusesTab from './StatusesTab.svelte'
  import LabelsTab from './LabelsTab.svelte'
  import PeopleTab from './PeopleTab.svelte'
  import AgentsTab from './AgentsTab.svelte'
  import SyncTab from './SyncTab.svelte'
  import { SETTINGS_TABS, SETTINGS_TAB_LABEL, type SettingsTab } from './board-settings'

  // One place for everything about a board: rename, sharing, agent policy, the
  // ClickUp sync, and the danger zone (archive / delete). Keeps the board
  // header uncluttered.
  let {
    board,
    open,
    initialTab,
    onTab,
    onClose,
    onArchived,
    onDeleted,
  }: {
    board: Board
    open: boolean
    /** The tab to show when the modal opens, and whenever it changes while
     *  open — the board route's `?settings=<tab>` (board-settings.ts). */
    initialTab?: SettingsTab
    /** A tab picked in the strip, so the route can follow it. */
    onTab?: (tab: SettingsTab) => void
    onClose: () => void
    onArchived: () => void
    onDeleted: () => void
  } = $props()

  let tab = $state<SettingsTab>('general')
  // The modal stays mounted between opens, so the tab is set ON OPEN (and when
  // the deep link changes under an open modal), not once at mount: a door that
  // opens it on Sync must land on Sync the second time too.
  $effect(() => {
    if (open) tab = initialTab ?? 'general'
  })
  const pick = (t: SettingsTab) => {
    tab = t
    onTab?.(t)
  }
  const isOwner = $derived(board.role === 'owner')
  // The Agents tab renders read-only for viewers and carries the access
  // request queue for editors — the policy write itself stays owner/editor.
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
</script>

<Modal {open} {onClose} title="Board settings" width="max-w-3xl" height="min-h-[32rem]">
  <!-- NOT `<Tabs>` — the parity rule in docs/UI-CONVENTIONS.md, not an oversight.
       This strip is one bordered GROUP of EQUAL cells (`flex-1`) spanning
       the modal; Tabs lays its cells out at content width with no group frame,
       and its sliding mark carries a hairline these cells don't. The kit wants a
       `Tabs` variant (fill cells + group border), not a call-site override. -->
  <!-- Below `sm` the cells sit in rows of three: six (or more) equal cells in a
       phone-width modal would cut every label short. -->
  <div class="mb-4 grid grid-cols-3 gap-1 rounded-md border border-line p-0.5 sm:flex">
    {#each SETTINGS_TABS as t (t)}
      <button
        aria-pressed={tab === t}
        data-settings-tab={t}
        onclick={() => pick(t)}
        class={cn(
          'rounded-md px-3 py-1.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors sm:flex-1',
          tab === t ? 'bg-raised text-fg' : 'text-muted hover:text-fg',
        )}
      >
        {SETTINGS_TAB_LABEL[t]}
      </button>
    {/each}
  </div>

  {#if tab === 'general'}
    <GeneralTab {board} {isOwner} {onClose} {onArchived} {onDeleted} />
  {/if}
  {#if tab === 'statuses'}<StatusesTab {board} />{/if}
  {#if tab === 'labels'}<LabelsTab {board} />{/if}
  {#if tab === 'people'}<PeopleTab {board} {isOwner} />{/if}
  {#if tab === 'agents'}<AgentsTab {board} {canEdit} />{/if}
  {#if tab === 'sync'}<SyncTab {board} />{/if}
</Modal>
