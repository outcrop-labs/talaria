<script lang="ts">
  import Modal from '@/components/ui/Modal.svelte'
  import { cn } from '@/lib/cn'
  import type { Board } from '@/lib/boards.svelte'
  import GeneralTab from './GeneralTab.svelte'
  import StatusesTab from './StatusesTab.svelte'
  import LabelsTab from './LabelsTab.svelte'
  import PeopleTab from './PeopleTab.svelte'
  import AgentsTab from './AgentsTab.svelte'

  type Tab = 'general' | 'statuses' | 'labels' | 'people' | 'agents'

  // One place for everything about a board: rename, sharing, agent policy, and the
  // danger zone (archive / delete). Keeps the board header uncluttered.
  let {
    board,
    open,
    onClose,
    onArchived,
    onDeleted,
  }: {
    board: Board
    open: boolean
    onClose: () => void
    onArchived: () => void
    onDeleted: () => void
  } = $props()

  let tab = $state<Tab>('general')
  const isOwner = $derived(board.role === 'owner')

  const tabs: Tab[] = ['general', 'statuses', 'labels', 'people', 'agents']
</script>

<Modal {open} {onClose} title="Board settings" width="max-w-xl">
  <div class="mb-4 flex gap-1 rounded-md border border-line p-0.5">
    {#each tabs as t (t)}
      <button
        onclick={() => (tab = t)}
        class={cn(
          'flex-1 rounded-md px-3 py-1.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
          tab === t ? 'bg-raised text-fg' : 'text-muted hover:text-fg',
        )}
      >
        {t}
      </button>
    {/each}
  </div>

  {#if tab === 'general'}
    <GeneralTab {board} {isOwner} {onClose} {onArchived} {onDeleted} />
  {/if}
  {#if tab === 'statuses'}<StatusesTab {board} />{/if}
  {#if tab === 'labels'}<LabelsTab {board} />{/if}
  {#if tab === 'people'}<PeopleTab {board} />{/if}
  {#if tab === 'agents'}<AgentsTab {board} />{/if}
</Modal>
