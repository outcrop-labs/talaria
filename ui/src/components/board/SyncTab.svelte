<script lang="ts">
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import type { Board } from '@/lib/boards.svelte'
  import { ago, useBoardSync } from '@/lib/board-sync'
  import { useClock } from '@/lib/clock.svelte'
  import SyncConnected from './SyncConnected.svelte'
  import SyncSetup from './SyncSetup.svelte'

  // The Sync tab of BoardSettingsModal.svelte — the board's ClickUp sync
  // (docs/BOARD-SYNC.md, docs/user/boards-clickup-sync.md). Not connected: an
  // owner gets the setup steps, an editor the sentence that only an owner
  // connects. Connected: the status, problems, mapping and doors, with the
  // owner-only ones offered to owners only.
  let { board }: { board: Board } = $props()

  const query = useBoardSync(() => board.id)
  const clock = useClock()
  const isOwner = $derived(board.role === 'owner')
  const data = $derived(query.data)
</script>

<div data-sync-tab>
  {#if query.isError && data === undefined}
    <QueryError error={query.error} title="Could not load the ClickUp sync" onRetry={() => void query.refetch()} />
  {:else if data === undefined}
    <SkeletonRows rows={5} />
  {:else if data.sync}
    <SyncConnected {board} sync={data.sync} />
  {:else}
    {#if data.disconnected}
      <p class="mb-4 rounded-md border border-line px-3 py-2 font-sans text-xs text-muted" data-sync-disconnected>
        Disconnected from {data.disconnected.listPath ?? 'a ClickUp list'}{data.disconnected.disconnectedAt
          ? ` ${ago(data.disconnected.disconnectedAt, clock.now)}`
          : ''}. Its tickets stay here, editable. Connecting the same list again re-attaches them with no duplicates and
        fills in the old mapping.
      </p>
    {/if}
    {#if isOwner}
      <SyncSetup {board} />
    {:else}
      <div class="space-y-2 font-sans text-sm text-muted" data-sync-owner-only>
        <p>This board is not synced from ClickUp.</p>
        <p>A board owner can connect one ClickUp list here: each task becomes a ticket on this board and keeps following ClickUp.</p>
      </div>
    {/if}
  {/if}
</div>
