<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { SettingsGlyph, COG } from '@/components/ui/settings-cog'
  import type { Board } from '@/lib/boards.svelte'
  import {
    BOARD_CHANNELS,
    BOARD_PREFS,
    NOTIFY_BOARDS_KEY,
    boardNotifyKey,
    clearBoardNotify,
    levelWords,
    putBoardNotify,
    useBoardNotify,
    type BoardNotifyAnswer,
    type BoardNotifyWrite,
  } from '@/lib/board-notify'
  import { useNotifications } from '@/lib/notifications'
  import { featureSet } from '@/lib/notify-center'
  import { p } from '@/router'
  import BoardNotifyForm from './BoardNotifyForm.svelte'

  // The Notifications tab of BoardSettingsModal.svelte: this person's own
  // setting for this board — every role has one, a viewer included, and it
  // is the only tab a viewer's modal shows. The bell beside the board's gear
  // sets the four presets in one click; this is the rest: Custom events and
  // which tickets, and the board's own Alerts and Email (`board-channels`).
  // The server holds it (lib/board-notify.ts); a write puts its answer into
  // the query the bell reads too.
  let { board }: { board: Board } = $props()

  const qc = useQueryClient()
  const notifications = useNotifications()
  const features = $derived(featureSet(notifications.data))
  const served = $derived(features.has(BOARD_PREFS))
  const query = useBoardNotify(
    () => board.id,
    () => served,
  )
  const data = $derived(query.data)

  const land = (r: BoardNotifyAnswer) => {
    qc.setQueryData(boardNotifyKey(board.id), r)
    void qc.invalidateQueries({ queryKey: NOTIFY_BOARDS_KEY })
  }
  const write = async (w: BoardNotifyWrite) => land(await putBoardNotify(board.id, w))
  const reset = async () => land(await clearBoardNotify(board.id))
</script>

<div data-board-notify-tab>
  {#if notifications.data && !served}
    <p class="font-sans text-sm text-muted">This instance does not offer a notification setting per board yet.</p>
  {:else if query.isError && data === undefined}
    <QueryError error={query.error} title="Could not load your notifications for this board" onRetry={() => void query.refetch()} />
  {:else if data === undefined}
    <SkeletonRows rows={5} />
  {:else}
    <BoardNotifyForm
      value={data}
      registry={data.registry}
      channelsServed={features.has(BOARD_CHANNELS)}
      subject="board"
      onWrite={write}
      onReset={reset}
      resetLabel="Use my default"
      bare
    />
    <p class="mt-4 flex flex-wrap items-center gap-x-1.5 font-sans text-[13px] leading-5 text-muted" data-board-notify-default>
      Your default for every board: {levelWords(data.default.effective, data.registry)}.
      <a
        href={p('/settings/:tab', { params: { tab: 'notifications' } })}
        class="inline-flex items-center gap-1 text-accent underline-offset-2 hover:underline"
      >
        <SettingsGlyph size={COG.sm} />Notification settings
      </a>
    </p>
  {/if}
</div>
