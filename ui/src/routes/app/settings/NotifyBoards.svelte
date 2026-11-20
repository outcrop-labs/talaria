<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import BoardNotifyForm from '@/components/board/BoardNotifyForm.svelte'
  import MenuValueButton from '@/components/notify/MenuValueButton.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cogMenuIcon } from '@/components/ui/settings-cog'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import {
    BOARD_CHANNELS,
    BOARD_PREFS,
    NOTIFY_BOARDS_KEY,
    QUICK_PRESETS,
    clearBoardNotify,
    levelWords,
    presetChoice,
    presetLabel,
    putBoardNotify,
    refusalMessage,
    saveBoardDefault,
    sourceWords,
    useNotifyBoards,
    type BoardNotifyWrite,
    type NotifyBoardRow,
  } from '@/lib/board-notify'
  import { navigate } from '@/router'

  // Settings → Notifications → Boards: the person's default for every board
  // (the same form as a board's Notifications tab), then every board they
  // can see with the setting that applies to it and where it comes from —
  // the boards with a setting of their own first, each with a quick change
  // and "Use my default". The server holds all of it (lib/board-notify.ts):
  // a write here is the write the board's bell and tab make, and the queries
  // that show it are invalidated together. Shown only when the api serves
  // `board-prefs`.
  let { features }: { features: ReadonlySet<string> } = $props()

  const qc = useQueryClient()
  const served = $derived(features.has(BOARD_PREFS))
  const query = useNotifyBoards(() => served)
  const data = $derived(query.data)
  const autosave = useAutosave(() => served)
  let rowError = $state<string | null>(null)
  let busy = $state<string | null>(null)

  // A change anywhere here can move every board's effective setting (the
  // default) or one board's: refetch the list, the open board's bell and tab,
  // and the notifications payload that carries the default.
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: NOTIFY_BOARDS_KEY })
    void qc.invalidateQueries({ queryKey: ['board-notify'] })
    void qc.invalidateQueries({ queryKey: ['notifications'] })
  }
  const writeDefault = async (w: BoardNotifyWrite | null) => {
    await saveBoardDefault(w)
    await refresh()
  }

  const rows = $derived.by((): NotifyBoardRow[] => {
    const all = data?.boards ?? []
    return [...all.filter((b) => b.source === 'board'), ...all.filter((b) => b.source !== 'board')]
  })

  async function onRow(id: string, run: () => Promise<unknown>) {
    if (busy) return
    rowError = null
    busy = id
    try {
      await run()
      await refresh()
      autosave.flash(id)
    } catch (e) {
      rowError = refusalMessage(e)
    } finally {
      busy = null
    }
  }

  const openOnBoard = (id: string) =>
    void navigate('/boards/:boardId', { params: { boardId: id }, search: { settings: 'notifications' } })

  const items = (b: NotifyBoardRow): ContextMenuEntry[] => {
    const registry = data?.registry
    const entries: ContextMenuEntry[] = QUICK_PRESETS.map((p) => ({
      label: presetLabel(registry, p),
      checked: b.effective.preset === p,
      // The checked preset is no change: a PUT would pin an inherited default.
      onSelect: () => {
        const w = presetChoice(b.effective, p, registry)
        if (w) void onRow(b.id, () => putBoardNotify(b.id, w))
      },
    }))
    if (b.source === 'board') {
      entries.push('sep', { label: 'Use my default', onSelect: () => void onRow(b.id, () => clearBoardNotify(b.id)) })
    }
    entries.push('sep', { label: 'Notification settings', icon: cogMenuIcon, title: `Open ${b.name}’s Notifications tab`, onSelect: () => openOnBoard(b.id) })
    return entries
  }
</script>

{#if served}
  <div data-notify-boards>
    <SettingsSection
      title="Boards"
      description="What each board’s tickets bring you. A board without a setting of its own follows your default."
      info="Off silences a board’s assignments and @mentions too. Approvals, and tickets you watch yourself, still reach you. Your choices change only what reaches you, never anyone else."
    >
      {#if query.isError && data === undefined}
        <QueryError variant="compact" error={query.error} title="Could not load your boards’ notification settings" onRetry={() => void query.refetch()} />
      {:else if data === undefined}
        <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={4} /></div></SettingsGroup>
      {:else}
        {#if data.registry}
          <div class="mb-5" data-notify-boards-default>
            <h3 class="mb-2 font-sans text-[13px] font-semibold leading-5 text-fg">Your default for every board</h3>
            <BoardNotifyForm
              value={data.default}
              registry={data.registry}
              channelsServed={features.has(BOARD_CHANNELS)}
              subject="default"
              onWrite={(w) => writeDefault(w)}
              onReset={() => writeDefault(null)}
              resetLabel="Back to the standard default"
            />
          </div>
        {/if}
        {#if rows.length === 0}
          <EmptyState variant="compact" title="No boards yet" hint="A board you join follows your default until you set it." />
        {:else}
          <SettingsGroup data-notify-boards-list>
            {#each rows as b (b.id)}
              <div data-notify-board={b.id} data-source={b.source} data-preset={b.effective.preset}>
                <SettingsRow
                  label={b.name}
                  wrap
                  hint={`${sourceWords(b.source)} · ${levelWords(b.effective, data.registry)}`}
                  saved={autosave.saved(b.id)}
                >
                  <DropdownMenu align="right" items={() => items(b)}>
                    {#snippet trigger(open)}
                      <MenuValueButton
                        {open}
                        class="w-[220px]"
                        disabled={busy === b.id}
                        label={presetLabel(data.registry, b.effective.preset)}
                        aria-label={`${b.name} notifications: ${levelWords(b.effective, data.registry)}`}
                        title={`${b.name} notifications: ${levelWords(b.effective, data.registry)}`}
                      />
                    {/snippet}
                  </DropdownMenu>
                </SettingsRow>
              </div>
            {/each}
          </SettingsGroup>
        {/if}
        {#if rowError}<p class="mt-2 font-sans text-[13px] leading-5 text-danger" role="alert">{rowError}</p>{/if}
      {/if}
    </SettingsSection>
  </div>
{/if}
