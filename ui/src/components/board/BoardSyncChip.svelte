<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ExternalLink, RefreshCw } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { SettingsGlyph, COG } from '@/components/ui/settings-cog'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import type { BoardRole } from '@/lib/boards.svelte'
  import {
    DOT_FOR_TONE,
    ago,
    chipView,
    lastRunNote,
    pausedHint,
    syncKey,
    syncNow,
    until,
    updateBoardSync,
    type BoardSync,
    type BoardSyncStatus,
  } from '@/lib/board-sync'
  import { useClock } from '@/lib/clock.svelte'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import SyncProblems from './SyncProblems.svelte'

  // THE BOARD'S SYNC, AT A GLANCE — beside the settings cog, for every member
  // of a board that syncs from ClickUp. A value-faced control: it shows the
  // sync's state ("ClickUp · synced 12m ago", "syncing", "needs a new token"…)
  // and opens a panel with last and next sync, the run's phase, warnings, the
  // top problems, Sync now (owners and editors) and the door into Board
  // settings → Sync (owners and editors: viewers cannot open the modal). It is
  // never silently stale: every state the projection can be in has words, and
  // a failure is red for everyone on the board. A paused sync says who paused
  // it and when; nothing syncs then, so an owner gets Resume in place of Sync
  // now and an editor a Sync now that says why it is off.
  //
  // The chip shrinks with its row (min-w-0 + truncate): on a phone-width board
  // the label cuts, never the chip, so its colour and problem count stay in
  // view.
  let {
    boardId,
    role,
    sync,
    onSettings,
  }: {
    boardId: string
    role: BoardRole
    sync: BoardSync
    /** Opens Board settings on the Sync tab; absent for a viewer. */
    onSettings?: () => void
  } = $props()

  const qc = useQueryClient()
  const clock = useClock()
  const isOwner = $derived(role === 'owner')
  const view = $derived(chipView(sync, clock.now, { canResume: isOwner }))
  const canSync = $derived(role === 'owner' || role === 'editor')
  const paused = $derived(sync.state === 'paused')
  const problems = $derived(sync.problemCount)
  // The accessible name and the tooltip are one string (FR4's rule for a
  // value-faced control): which control, then its value.
  const name = $derived(
    `ClickUp sync: ${view.text}${problems ? `, ${problems} problem${problems === 1 ? '' : 's'}` : ''}`,
  )
  const summary = $derived(lastRunNote(sync.lastRun?.result))
  let busy = $state(false)

  async function runNow() {
    busy = true
    try {
      const r = await syncNow(boardId)
      if (r.live) pushToast({ title: 'A sync is already running', body: 'Its progress shows here.', tone: 'info' })
    } catch (e) {
      toastError('Could not start a sync', e)
    } finally {
      busy = false
      void qc.invalidateQueries({ queryKey: syncKey(boardId) })
    }
  }

  async function resume() {
    busy = true
    try {
      const r = await updateBoardSync(boardId, { paused: false })
      qc.setQueryData<BoardSyncStatus>(syncKey(boardId), { sync: r.sync, disconnected: null })
      pushToast({ title: 'Sync resumed', body: 'The schedule is back on, and Sync now works again.', tone: 'success' })
    } catch (e) {
      toastError('Could not resume the sync', e)
    } finally {
      busy = false
      void qc.invalidateQueries({ queryKey: syncKey(boardId) })
    }
  }

  const TONE_CLASS: Record<string, string> = {
    ok: 'border-line text-muted hover:text-fg',
    live: 'border-[var(--theme-accent-border)] text-accent',
    warn: 'border-warning/50 text-warning',
    danger: 'border-danger/50 text-danger',
    idle: 'border-line text-muted hover:text-fg',
  }
</script>

<!-- The row's flex item: min-w-0 lets it shrink below the label's width. -->
<div class="min-w-0 max-w-full" data-sync-chip-root>
  <Popover align="right" class="w-80 p-3">
    {#snippet trigger(open)}
      <button
        type="button"
        title={name}
        aria-label={name}
        aria-expanded={open}
        aria-haspopup="dialog"
        data-sync-chip={view.tone}
        class={cn(
          'flex h-8 min-w-0 max-w-[min(16rem,100%)] items-center gap-1.5 rounded-md border px-2 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
          focusGold,
          TONE_CLASS[view.tone],
          open && 'bg-raised text-fg',
        )}
      >
        <RefreshCw size={12} class="shrink-0" />
        <span class="truncate">ClickUp · {view.text}</span>
        {#if problems > 0}
          <span class="shrink-0 rounded bg-warning/15 px-1 text-warning" aria-hidden="true">{problems}</span>
        {/if}
      </button>
    {/snippet}
    {#snippet content(close)}
      <div class="space-y-3" data-sync-chip-panel>
        <div>
          <div class="flex items-center gap-2">
            <StatusDot status={DOT_FOR_TONE[view.tone]} pulse={view.tone === 'live'} />
            <span class="font-sans text-sm font-medium text-fg">ClickUp sync · {view.text}</span>
          </div>
          {#if sync.listPath}
            <div class="mt-0.5 min-w-0 truncate font-sans text-xs text-muted">
              {#if sync.listUrl}
                <a href={sync.listUrl} target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 underline-offset-2 hover:underline">
                  {sync.listPath} <ExternalLink size={10} />
                </a>
              {:else}{sync.listPath}{/if}
            </div>
          {/if}
          <div class="mt-1 font-sans text-xs text-muted">{view.detail}</div>
        </div>

        <dl class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 font-sans text-xs">
          <dt class="text-muted">Last sync</dt>
          <dd class="text-fg">{ago(sync.lastSyncedAt, clock.now) ?? 'not yet'}</dd>
          <dt class="text-muted">Next sync</dt>
          <dd class="text-fg">
            {#if paused}none while paused{:else if sync.intervalMinutes == null}only on Sync now{:else if !sync.schedulerArmed}scheduled syncs are off{:else}{until(sync.nextSyncAt, clock.now) ?? '—'}{/if}
          </dd>
          {#if summary}
            <dt class="text-muted">Last run</dt>
            <dd class="text-fg">{summary}</dd>
          {/if}
        </dl>

        {#each sync.warnings as w (w.kind)}
          <div class="rounded-md border border-warning/40 px-2.5 py-1.5 font-sans text-xs">
            <div class="text-fg">{w.detail}</div>
            <div class="text-muted">{w.fix}</div>
          </div>
        {/each}

        {#if sync.problems.length}
          <div>
            <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Problems ({problems})</div>
            <div class="max-h-64 overflow-y-auto">
              <SyncProblems problems={sync.problems} total={problems} {boardId} shown={3} onLeave={close} />
            </div>
          </div>
        {/if}

        {#if canSync}
          <div class="flex items-center justify-between gap-2 border-t border-line-subtle pt-2">
            {#if paused && isOwner}
              <Button variant="outline" size="sm" disabled={busy} data-sync-resume onclick={() => void resume()}>
                {busy ? 'Resuming…' : 'Resume'}
              </Button>
            {:else}
              <Button
                variant="outline"
                size="sm"
                disabled={busy || !!sync.liveRun || sync.state === 'needs_credential' || paused}
                title={sync.state === 'needs_credential' ? 'The sync needs a new token first — a board owner replaces it in Board settings → Sync' : paused ? pausedHint(sync, clock.now, false) : undefined}
                onclick={() => void runNow()}
              >
                {sync.liveRun ? 'Syncing…' : 'Sync now'}
              </Button>
            {/if}
            {#if onSettings}
              <Button
                variant="ghost"
                size="sm"
                data-sync-settings
                onclick={() => {
                  close()
                  onSettings?.()
                }}
              >
                <SettingsGlyph size={COG.sm} /> Sync settings
              </Button>
            {/if}
          </div>
        {/if}
      </div>
    {/snippet}
  </Popover>
</div>
