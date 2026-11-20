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
  import NotificationsTab from './NotificationsTab.svelte'
  import { SETTINGS_TAB_LABEL, tabsFor, type SettingsTab } from './board-settings'

  // One place for everything about a board: rename, sharing, agent policy, the
  // ClickUp sync, and the danger zone (archive / delete) — and each person's
  // own notification setting for it. Keeps the board header uncluttered.
  //
  // WHO SEES WHICH TAB (board-settings.ts `tabsFor`): owners and editors see
  // every tab; a viewer sees only Notifications, their own setting — the
  // board's configuration is not theirs to change, and General and People are
  // not read-only views. A viewer reaches it from the bell's "Notification
  // settings" (`?settings=notifications`), so for them the modal is that
  // setting and is named for it.
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

  const tabs = $derived(tabsFor(board.role))
  let tab = $state<SettingsTab>('general')
  // The modal stays mounted between opens, so the tab is set ON OPEN (and when
  // the deep link changes under an open modal), not once at mount: a door that
  // opens it on Sync must land on Sync the second time too. A tab the role
  // does not see falls back to its first.
  $effect(() => {
    if (open) tab = initialTab && tabs.includes(initialTab) ? initialTab : (tabs[0] ?? 'notifications')
  })
  // What is drawn: never a tab the role does not see, even for the moment
  // before the effect above has run.
  const shown = $derived(tabs.includes(tab) ? tab : tabs[0])
  const pick = (t: SettingsTab) => {
    tab = t
    onTab?.(t)
  }
  const isOwner = $derived(board.role === 'owner')
  // AgentsTab can render read-only and carries the access request queue for
  // editors; a viewer is not shown the tab at all (`tabsFor`).
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
</script>

<Modal {open} {onClose} title={tabs.length > 1 ? 'Board settings' : 'Notification settings'} width="max-w-3xl" height="min-h-[32rem]">
  <!-- NOT `<Tabs>` — the parity rule in docs/UI-CONVENTIONS.md, not an oversight.
       This strip is one bordered GROUP of EQUAL cells (`flex-1`) spanning
       the modal; Tabs lays its cells out at content width with no group frame,
       and its sliding mark carries a hairline these cells don't. The kit wants a
       `Tabs` variant (fill cells + group border), not a call-site override. -->
  <!-- Below `sm` the cells sit in rows of three: six (or more) equal cells in a
       phone-width modal would cut every label short. -->
  <!-- One tab (a viewer's) draws no strip: there is nothing to switch to. -->
  {#if tabs.length > 1}
  <div class="mb-4 grid grid-cols-3 gap-1 rounded-md border border-line p-0.5 sm:flex">
    {#each tabs as t (t)}
      <button
        aria-pressed={shown === t}
        data-settings-tab={t}
        onclick={() => pick(t)}
        class={cn(
          'rounded-md px-3 py-1.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors sm:flex-1',
          shown === t ? 'bg-raised text-fg' : 'text-muted hover:text-fg',
        )}
      >
        {SETTINGS_TAB_LABEL[t]}
      </button>
    {/each}
  </div>
  {/if}

  {#if shown === 'general'}
    <GeneralTab {board} {isOwner} {onClose} {onArchived} {onDeleted} />
  {/if}
  {#if shown === 'statuses'}<StatusesTab {board} />{/if}
  {#if shown === 'labels'}<LabelsTab {board} />{/if}
  {#if shown === 'people'}<PeopleTab {board} {isOwner} />{/if}
  {#if shown === 'agents'}<AgentsTab {board} {canEdit} />{/if}
  {#if shown === 'sync'}<SyncTab {board} />{/if}
  {#if shown === 'notifications'}<NotificationsTab {board} />{/if}
</Modal>
