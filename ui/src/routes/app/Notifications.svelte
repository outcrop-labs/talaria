<script lang="ts">
  import { CheckCheck, Keyboard, Undo2 } from '@lucide/svelte'
  import { searchParams } from 'sv-router'
  import { untrack } from 'svelte'
  import { SvelteSet } from 'svelte/reactivity'
  import PageSurface from '@/components/app/PageSurface.svelte'
  import NotificationList from '@/components/notify/NotificationList.svelte'
  import NotifyFilters from '@/components/notify/NotifyFilters.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Kbd from '@/components/ui/Kbd.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import { putJson } from '@/lib/fetch-json'
  import {
    badgeCount,
    centerQuery,
    featureSet,
    filtersActive,
    filtersFrom,
    flattenPages,
    hasInboxV2,
    inView,
    matchesFilters,
    readToggleIds,
    useCachedPictures,
    useCenterList,
    useInboxActions,
    useNotifySettings,
    viewFrom,
    VIEW_LABEL,
    type CenterFilters,
    type CenterRow,
    type CenterView,
  } from '@/lib/notify-center'
  import { isEditableTarget, KEY_HELP, keyAction, moveFocus, refocus, targetsOf, toggleSelected } from '@/lib/notify-keys'
  import { openNotification } from '@/lib/notify-open'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import { navigate, navigateHref } from '@/router'

  // /notifications — the notification center, the bell's full page. Views
  // For you / All / Done, filters, keyset paging ("Load more", and more as you
  // scroll), selection and the keyboard (notify-keys.ts). THE URL IS THE
  // SELECTION: `?view=done&unread=1&category=direct&kind=dm` is a link.
  //
  // Everything past wave 1's list is the inbox API's (W2-H, `inbox-v2`):
  // without it there is no Done view and no paging, and For you / All are the
  // newest 50 sliced and filtered here — the same rows the bell holds.
  claimViewTitle('Notifications')

  const base = useNotifySettings()
  const actions = useInboxActions()
  const pictures = useCachedPictures()
  const features = $derived(featureSet(base.data))
  const v2 = $derived(hasInboxV2(features))
  const classes = $derived(base.data?.classes)

  const param = (k: string): string | null => {
    const v = searchParams.get(k)
    return v === null || v === undefined || typeof v === 'boolean' ? null : String(v)
  }
  const requested = $derived(viewFrom(param('view')))
  // Done needs the inbox API; a link to it on an older api lands on For you.
  const view: CenterView = $derived(requested === 'done' && !v2 ? 'foryou' : requested)
  const filters: CenterFilters = $derived(filtersFrom(param))

  /** A view or filter change is one navigation (one history entry). */
  const go = (v: CenterView, f: CenterFilters) => {
    const q = new URLSearchParams(centerQuery(v, f))
    q.delete('limit')
    if (v === 'foryou') q.delete('view')
    const s = q.toString()
    void navigate('/notifications', { search: s ? `?${s}` : '' })
  }

  const list = useCenterList(() => ({ view, filters, enabled: v2 }))
  const hidden = new SvelteSet<string>()
  const rows: CenterRow[] = $derived.by(() => {
    const all: CenterRow[] = v2 ? flattenPages(list.data?.pages ?? []) : (base.data?.notifications ?? []).filter((r) => matchesFilters(r, filters))
    return all.filter((r) => inView(r, view) && !hidden.has(r.id))
  })
  const ids = $derived(rows.map((r) => r.id))
  const pending = $derived(v2 ? list.isPending : base.isPending)
  // Each query's flags read on their own line: a short-circuit across two
  // queries would leave the second unread, and unsubscribed.
  const listFailed = $derived(list.isError && !list.data)
  const baseFailed = $derived(base.isError && !base.data)
  const failed = $derived(v2 ? listFailed : baseFailed)
  const hasMore = $derived(v2 && !!list.hasNextPage)

  // ── Focus and selection ────────────────────────────────────────────────────
  let focusIndex = $state(-1)
  let focusedId = $state<string | null>(null)
  let selection = $state<Set<string>>(new Set())
  let helpOpen = $state(false)

  // The first row is focused as soon as there is one (j then moves from it),
  // and the focus stays on its row — or its place — when the list changes.
  $effect(() => {
    const now = ids
    untrack(() => {
      if (focusedId === null && focusIndex === -1) {
        if (now.length) {
          focusIndex = 0
          focusedId = now[0] ?? null
        }
        return
      }
      const next = refocus(focusedId, focusIndex, now)
      focusIndex = next
      focusedId = next >= 0 ? (now[next] ?? null) : null
    })
  })

  // A view or filter change starts the selection afresh.
  $effect(() => {
    void view
    void filters
    selection = new Set()
  })

  const focusRow = (i: number) => {
    focusIndex = i
    focusedId = ids[i] ?? null
    if (focusedId) document.querySelector(`[data-notification-row="${focusedId}"]`)?.scrollIntoView({ block: 'nearest' })
    // Near the end of what is loaded: fetch the next page before j runs out.
    if (hasMore && i >= ids.length - 5 && !list.isFetchingNextPage) void list.fetchNextPage()
  }

  const rowsOf = (targets: string[]) => rows.filter((r) => targets.includes(r.id))

  // ── Actions ────────────────────────────────────────────────────────────────
  const open = (r: CenterRow) =>
    void openNotification(
      { id: r.id, href: r.href },
      {
        navigate: (path) => navigateHref(path),
        markRead: (ids) => putJson('/api/notifications', { ids }),
        invalidate: actions.refresh,
        origin: location.origin,
      },
    )

  // UNDO. Done is one key (and Shift+E is every row shown), so each Done or
  // Move to inbox can be taken back for a few seconds — the bar under the
  // header, or `z`. An undo of Done puts the rows back exactly: in the inbox,
  // and unread again where they were unread (Done also reads, O-11).
  // Raw, not a deep proxy: the record is replaced whole, and `ids` keeps its
  // identity for the failure check below. `write` is the action being undone:
  // the undo waits for it, so the inverse never reaches the server first.
  const UNDO_MS = 10_000
  type Undo = { ids: string[]; unread: string[]; restored: boolean; write: Promise<boolean> }
  let undo = $state.raw<Undo | null>(null)
  let undoTimer: ReturnType<typeof setTimeout> | undefined
  const offerUndo = (next: Undo | null) => {
    clearTimeout(undoTimer)
    undo = next
    if (next) undoTimer = setTimeout(() => (undo = null), UNDO_MS)
  }
  $effect(() => () => clearTimeout(undoTimer))

  const done = (targets: string[]) => {
    if (!targets.length || !v2) return
    const restored = view === 'done'
    const wasUnread = rowsOf(targets).filter((r) => !r.readAt).map((r) => r.id)
    for (const id of targets) hidden.add(id)
    selection = new Set([...selection].filter((id) => !targets.includes(id)))
    const write = restored ? actions.restore(targets) : actions.done(targets)
    offerUndo({ ids: targets, unread: wasUnread, restored, write })
    void write.then((ok) => {
      for (const id of targets) hidden.delete(id)
      // A write that failed has nothing to undo (its toast says why).
      if (!ok && undo?.ids === targets) offerUndo(null)
    })
  }

  const runUndo = () => {
    const u = undo
    if (!u) return
    offerUndo(null)
    void u.write.then(async (landed) => {
      if (!landed) return
      if (u.restored) {
        await actions.done(u.ids)
        return
      }
      if ((await actions.restore(u.ids)) && u.unread.length) await actions.unread(u.unread)
    })
  }

  const toggleRead = (targets: string[]) => {
    if (!targets.length || view === 'done') return
    const { read, unread } = readToggleIds(rowsOf(targets))
    if (read.length) void actions.read(read)
    else if (v2) void actions.unread(unread)
  }

  const onKey = (e: KeyboardEvent) => {
    if (helpOpen) {
      if (e.key === '?' || e.key === 'Escape') {
        e.preventDefault()
        helpOpen = false
      }
      return
    }
    // A modal dialog that is open owns the keyboard.
    if (document.querySelector('[aria-modal="true"]')) return
    // So does anything outside this page that has the focus — the bell's or
    // the account menu's panel, the dock: `e` typed there is not a Done here.
    const t = e.target as Node | null
    if (t && t !== document.body && t !== document.documentElement && pageEl && !pageEl.contains(t)) return
    const a = keyAction({
      key: e.key,
      shiftKey: e.shiftKey,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      altKey: e.altKey,
      editable: isEditableTarget(e.target),
    })
    if (!a) return
    // Enter on a focused button is that button's own click.
    if (a === 'open' && e.key === 'Enter' && (e.target as HTMLElement | null)?.closest?.('button, a')) return
    e.preventDefault()
    const ctx = { focused: focusedId, selection, visible: ids }
    switch (a) {
      case 'next':
      case 'prev':
        focusRow(moveFocus(focusIndex, ids.length, a))
        break
      case 'open': {
        const r = rows[focusIndex]
        if (r) open(r)
        break
      }
      case 'done':
      case 'doneAll':
        done(targetsOf(a, ctx))
        break
      case 'toggleRead':
        toggleRead(targetsOf('toggleRead', ctx))
        break
      case 'select':
        if (focusedId) selection = toggleSelected(selection, focusedId)
        break
      case 'help':
        helpOpen = true
        break
      case 'undo':
        runUndo()
        break
      case 'escape':
        selection = new Set()
        break
    }
  }

  // ── Infinite scroll ────────────────────────────────────────────────────────
  let pageEl = $state<HTMLDivElement | null>(null)
  let sentinel = $state<HTMLDivElement | null>(null)
  $effect(() => {
    if (!sentinel || !hasMore) return
    const io = new IntersectionObserver((entries) => {
      if (entries.some((x) => x.isIntersecting) && list.hasNextPage && !list.isFetchingNextPage) void list.fetchNextPage()
    })
    io.observe(sentinel)
    return () => io.disconnect()
  })

  const tabs = $derived(
    (v2 ? (['foryou', 'all', 'done'] as const) : (['foryou', 'all'] as const)).map((id) => ({ id, label: VIEW_LABEL[id] })),
  )
  const forYouUnread = $derived(badgeCount(base.data))
  const selected = $derived(rows.filter((r) => selection.has(r.id)))

  const empty = $derived.by((): { title: string; hint?: string } => {
    if (filtersActive(filters)) return { title: 'No notifications match these filters' }
    if (view === 'done') return { title: 'Nothing marked done', hint: 'Mark a notification done (e) to clear it from your inbox. It stays here.' }
    if (view === 'foryou') return { title: 'Nothing needs you', hint: 'Mentions, direct messages and anything waiting on you land here.' }
    return { title: 'All caught up' }
  })
</script>

<svelte:window onkeydown={onKey} />

<PageSurface>
  <div bind:this={pageEl}>
  <div class="mb-4 flex flex-wrap items-center gap-3">
    <Tabs items={tabs} value={view} onChange={(v) => go(v, filters)} />
    {#if forYouUnread}
      <span class="font-mono text-[10px] text-accent" data-center-unread>{forYouUnread} unread for you</span>
    {/if}
    <div class="ml-auto flex items-center gap-1">
      {#if (base.data?.unread ?? 0) > 0 && view !== 'done'}
        <IconButton title="Mark all read" onclick={() => void actions.allRead()}>
          <CheckCheck size={15} strokeWidth={1.5} />
        </IconButton>
      {/if}
      <IconButton title="Keyboard shortcuts (?)" onclick={() => (helpOpen = true)}>
        <Keyboard size={15} strokeWidth={1.5} />
      </IconButton>
    </div>
  </div>
  <div class="mb-3">
    <NotifyFilters {filters} {classes} onChange={(f) => go(view, f)} />
  </div>

  {#if selected.length}
    <div class="mb-2 flex flex-wrap items-center gap-2 rounded-md border border-line bg-raised px-3 py-1.5 text-sm" data-center-selection={selected.length}>
      <span class="font-mono text-[11px] text-muted">{selected.length} selected</span>
      {#if v2}
        <Button size="xs" variant="outline" onclick={() => done(selected.map((r) => r.id))}>
          {view === 'done' ? 'Move to inbox' : 'Done'}
        </Button>
      {/if}
      {#if view !== 'done'}
        <Button size="xs" variant="outline" onclick={() => toggleRead(selected.map((r) => r.id))}>
          {selected.some((r) => !r.readAt) ? 'Mark read' : 'Mark unread'}
        </Button>
      {/if}
      <Button size="xs" variant="ghost" onclick={() => (selection = new Set())}>Clear</Button>
    </div>
  {/if}

  {#if undo}
    <div
      class="mb-2 flex flex-wrap items-center gap-2 rounded-md border border-line bg-raised px-3 py-1.5 text-sm"
      role="status"
      data-center-undo={undo.restored ? 'restored' : 'done'}
    >
      <span class="font-sans text-xs text-fg">
        {undo.ids.length === 1 ? '1 notification' : `${undo.ids.length} notifications`}
        {undo.restored ? 'moved back to the inbox.' : 'marked done.'}
      </span>
      <Button size="xs" variant="outline" onclick={runUndo} data-center-undo-button>
        <Undo2 size={13} strokeWidth={1.5} />Undo
      </Button>
      <span class="font-mono text-[10px] text-ink-dim">z</span>
    </div>
  {/if}

  <Panel class="p-0" data-center-view={view}>
    {#if pending}
      <div class="p-4"><SkeletonRows rows={8} /></div>
    {:else if failed}
      <div class="p-4">
        <QueryError
          variant="compact"
          error={v2 ? list.error : base.error}
          title="Could not load your notifications"
          onRetry={() => void (v2 ? list.refetch() : base.refetch())}
        />
      </div>
    {:else if rows.length === 0}
      <EmptyState variant="compact" title={empty.title} hint={empty.hint} />
    {:else}
      <div data-center-list>
        <NotificationList
          {rows}
          {classes}
          pictures={pictures.current}
          {focusedId}
          {selection}
          selectable
          canArchive={v2}
          canUnread={v2}
          onOpen={open}
          onDone={(r) => done([r.id])}
          onRestore={(r) => done([r.id])}
          onToggleRead={(r) => toggleRead([r.id])}
          onSelect={(r) => (selection = toggleSelected(selection, r.id))}
        />
      </div>
      <!-- A failed page leaves hasNextPage true, so its error shows above
           the button rather than instead of it. -->
      {#if v2 && list.isFetchNextPageError && !list.isFetchingNextPage}
        <div class="border-t border-line p-2">
          <QueryError variant="inline" error={list.error} title="Could not load more" onRetry={() => void list.fetchNextPage()} />
        </div>
      {/if}
      {#if hasMore}
        <div bind:this={sentinel} class="flex justify-center border-t border-line p-2">
          <Button size="sm" variant="ghost" disabled={list.isFetchingNextPage} onclick={() => void list.fetchNextPage()} data-center-more>
            {list.isFetchingNextPage ? 'Loading' : 'Load more'}
          </Button>
        </div>
      {/if}
    {/if}
  </Panel>
  {#if !v2 && base.data}
    <p class="mt-2 font-sans text-xs text-muted">Showing your newest {base.data.notifications.length} notifications.</p>
  {/if}
  </div>
</PageSurface>

<Modal open={helpOpen} onClose={() => (helpOpen = false)} title="Keyboard shortcuts">
  <ul class="space-y-2" data-center-help>
    {#each KEY_HELP as h (h.what)}
      <li class="flex items-center gap-3 text-sm text-fg">
        <span class="flex w-28 shrink-0 items-center gap-1">
          {#each h.keys as k (k)}<Kbd>{k}</Kbd>{/each}
        </span>
        <span class="text-muted">{h.what}</span>
      </li>
    {/each}
  </ul>
</Modal>
