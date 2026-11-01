<script lang="ts">
  import { Bell, BellOff, BellRing, CheckCheck } from '@lucide/svelte'
  import { SvelteSet } from 'svelte/reactivity'
  import NotificationList from '@/components/notify/NotificationList.svelte'
  import PauseMenu from '@/components/notify/PauseMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import {
    browserNotifyPrefOff,
    ensurePushSubscription,
    permissionState,
    setBrowserNotifyPref,
    turnOnDesktopAlerts,
    watchPermission,
    type PermissionState,
  } from '@/lib/browser-notify'
  import { inDesktopShell } from '@/lib/desktop-shell'
  import { putJson } from '@/lib/fetch-json'
  import { saveNotifyPrompt, useMarkNotificationsRead, useNotifications } from '@/lib/notifications'
  import {
    badgeCount,
    featureSet,
    flattenPages,
    hasInboxV2,
    inView,
    NO_FILTERS,
    readToggleIds,
    useCachedPictures,
    useCenterList,
    useInboxActions,
    useNotifyPatch,
    type CenterRow,
  } from '@/lib/notify-center'
  import { dndView, safeZone } from '@/lib/notify-dnd'
  import { blockedSteps, detectBrowser, insecureExplanation } from '@/lib/notify-help'
  import { openNotification } from '@/lib/notify-open'
  import { turnOnResult } from '@/lib/notify-prompt'
  import { readText, writeText } from '@/lib/persist'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigateHref, p } from '@/router'

  // The dock's bell: the notification inbox as a dropdown beside the account
  // chip, Slack/Linear-style since wave 2 — For you (Needs you + Direct) and
  // All, rows grouped by the thing they are about with their counts, Done and
  // read/unread on each row, the pause menu, and "View all" for the full
  // page (/notifications).
  //
  // THE BADGE COUNTS FOR YOU (O-10): `unreadForYou`, so the Following firehose
  // shows in All without badging; an api older than the contract has no such
  // field and the badge counts every unread row, as wave 1's did.
  //
  // The count follows the rail's `/home` doctrine: a failed read is `!`, never
  // a silent 0 — the bell is the one surface a person glances at to ask "is
  // anything waiting", and an outage must not answer "no".
  const query = useNotifications()
  const markRead = useMarkNotificationsRead()
  const actions = useInboxActions()
  const patch = useNotifyPatch()
  const pictures = useCachedPictures()
  const qc = useQueryClient()

  const features = $derived(featureSet(query.data))
  // Done, mark unread and paged views are the inbox API's (W2-H). Without
  // it the tabs slice the newest 50 this read already holds.
  const v2 = $derived(hasInboxV2(features))
  const badge: number | null = $derived(query.isError && query.data === undefined ? null : badgeCount(query.data))
  const badgeLabel = $derived(badge === null ? '!' : badge > 99 ? '99+' : String(badge))
  const badgeTitle = $derived(
    badge === null
      ? 'Could not load your notifications'
      : query.data?.unreadForYou !== undefined
        ? `${badge} unread for you`
        : `${badge} unread`,
  )
  const classes = $derived(query.data?.classes)

  // For you or All, remembered per browser (a convenience, never state that
  // must survive).
  type BellTab = 'foryou' | 'all'
  const TAB_KEY = 'talaria.bell.tab'
  let tab = $state<BellTab>(readText(TAB_KEY) === 'all' ? 'all' : 'foryou')
  const setTab = (t: BellTab) => {
    tab = t
    writeText(TAB_KEY, t)
  }

  // A summary toast or banner ("5 new — …") opens the bell itself.
  let popoverOpen = $state(false)
  $effect(() => {
    const show = () => (popoverOpen = true)
    window.addEventListener('talaria:open-notifications', show)
    return () => window.removeEventListener('talaria:open-notifications', show)
  })

  // The view's own read, only while the bell is open and only with the inbox
  // API: For you is its own server view, so a noisy Following stream can never
  // push the rows that need you out of the newest 50.
  const list = useCenterList(() => ({ view: tab, filters: NO_FILTERS, enabled: v2 && popoverOpen, limit: 30 }))
  // Rows marked Done here leave at once; the refetch confirms it.
  const hidden = new SvelteSet<string>()
  const rows: CenterRow[] = $derived.by(() => {
    const base: CenterRow[] = v2 && list.data ? flattenPages(list.data.pages) : (query.data?.notifications ?? [])
    return base.filter((r) => inView(r, tab) && !hidden.has(r.id))
  })
  const unreadHere = $derived(rows.filter((r) => !r.readAt).length)

  // DND: "Paused until 3:00 PM · Resume" — a pause is read from the stored
  // instant (it shows the moment it is set), a schedule from the server's
  // evaluation. Re-read every 30 s while open, so an expired pause goes.
  let now = $state(Date.now())
  $effect(() => {
    if (!popoverOpen) return
    now = Date.now()
    const t = setInterval(() => (now = Date.now()), 30_000)
    return () => clearInterval(t)
  })
  const dnd = $derived(query.data?.dnd)
  const dndNow = $derived(dndView(dnd, now, safeZone(dnd?.timeZone)))
  let pauseOpen = $state(false)
  let resumeError = $state<string | null>(null)
  const resume = async () => {
    resumeError = null
    const r = await patch({ dnd: { pausedUntil: null } })
    if ('error' in r) resumeError = r.error
    now = Date.now()
  }

  // THE DESKTOP-ALERTS ROW, at the top of the popover: the one place that
  // always says what this browser will do and how to change it — the soft
  // ask's card retires after three "Not now"s, this row never does. Never in
  // the desktop shell: the shell asks for itself (Settings → Desktop).
  const shell = inDesktopShell()
  const browser = typeof navigator !== 'undefined' ? detectBrowser(navigator.userAgent, { brave: 'brave' in navigator }) : null
  let perm = $state<PermissionState>(permissionState())
  let prefOff = $state(browserNotifyPrefOff())
  let stepsOpen = $state(false)
  let turnOnNote = $state<string | null>(null)
  $effect(() =>
    watchPermission((p) => {
      perm = p
      prefOff = browserNotifyPrefOff()
    }),
  )
  const steps = $derived(browser ? blockedSteps(browser) : null)

  function turnOn() {
    turnOnNote = null
    if (perm === 'granted') {
      // Granted already, switched off here: the switch is ours.
      setBrowserNotifyPref(true)
      prefOff = false
      void ensurePushSubscription()
      return
    }
    // Asked synchronously, inside this click (browser-notify.ts, rule 1).
    void turnOnDesktopAlerts().then(async (after) => {
      perm = after
      prefOff = browserNotifyPrefOff()
      const r = turnOnResult(after)
      if (r.kind === 'on') {
        await saveNotifyPrompt('granted')
        void qc.invalidateQueries({ queryKey: ['notifications'] })
      } else if (r.kind === 'closed' || r.kind === 'unavailable') turnOnNote = r.message
      else stepsOpen = true
    })
  }

  // A row's open is wave 1's one door (notify-open.ts): route in place — no
  // page load — and mark it read. The bell closes first: the person clicked to
  // GO there.
  function open(n: CenterRow, close: () => void) {
    close()
    void openNotification(
      { id: n.id, href: n.href },
      {
        navigate: (path) => navigateHref(path),
        markRead: (ids) => putJson('/api/notifications', { ids }),
        invalidate: actions.refresh,
        origin: location.origin,
      },
    )
  }

  // The write awaits its refetch, so once it settles the list itself says
  // where the row is, and the local hide can go (a failure brings it back).
  const done = (n: CenterRow) => {
    hidden.add(n.id)
    void actions.done([n.id]).then(() => hidden.delete(n.id))
  }

  const toggleRead = (n: CenterRow) => {
    const { read, unread } = readToggleIds([n])
    if (read.length) void actions.read(read)
    else void actions.unread(unread)
  }
</script>

<Popover align="right" offset={8} class="w-[400px] max-w-[calc(100vw-16px)]" bind:open={popoverOpen}>
  {#snippet trigger(open)}
    <button
      type="button"
      aria-expanded={open}
      aria-label="Notifications"
      data-notify-badge={badge === null ? 'error' : String(badge)}
      class="relative flex h-9 w-9 items-center justify-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
    >
      {#if dndNow.active}
        <BellOff size={16} strokeWidth={1.5} class="shrink-0" />
      {:else}
        <Bell size={16} strokeWidth={1.5} class="shrink-0" />
      {/if}
      {#if badge === null || badge > 0}
        <span
          title={badgeTitle}
          class="absolute right-0 top-0 font-mono text-[10px] font-medium leading-3 tracking-[0.05em] {badge === null ? 'text-[color:var(--theme-danger)]' : 'text-accent'}"
        >
          {badgeLabel}
        </span>
      {/if}
    </button>
  {/snippet}
  {#snippet content(close)}
    {#if !shell && (perm === 'default' || perm === 'denied' || perm === 'insecure' || (perm === 'granted' && prefOff))}
      <div class="border-b border-line px-3 py-2" data-notify-row={perm}>
        {#if perm === 'insecure'}
          <div class="flex items-start gap-2 font-sans text-xs text-muted">
            <BellOff size={14} strokeWidth={1.5} class="mt-0.5 shrink-0" />
            <span>{insecureExplanation(location.origin)}</span>
          </div>
        {:else if perm === 'denied'}
          <button
            type="button"
            aria-expanded={stepsOpen}
            onclick={() => (stepsOpen = !stepsOpen)}
            class="flex w-full items-start gap-2 text-left font-sans text-sm text-fg"
          >
            <BellOff size={14} strokeWidth={1.5} class="mt-1 shrink-0 text-muted" />
            <span>Desktop alerts are blocked in {browser?.name ?? 'this browser'}</span>
          </button>
          {#if stepsOpen && steps}
            <ul class="mt-1.5 list-disc space-y-1 pl-8 font-sans text-xs text-muted">
              {#each [...steps.browser, ...steps.os] as step (step)}<li>{step}</li>{/each}
            </ul>
            <div class="mt-1.5 pl-6 font-sans text-xs text-muted">{steps.closing}</div>
          {/if}
        {:else}
          <button
            type="button"
            onclick={turnOn}
            class="flex w-full items-start gap-2 rounded-md text-left font-sans text-sm text-accent transition-colors duration-[120ms] hover:text-fg"
          >
            <BellRing size={14} strokeWidth={1.5} class="mt-1 shrink-0" />
            <span>Turn on desktop alerts</span>
          </button>
          {#if turnOnNote}<div class="mt-1 pl-6 font-sans text-xs text-muted">{turnOnNote}</div>{/if}
        {/if}
      </div>
    {/if}

    {#if dndNow.active}
      <div class="flex items-center gap-2 border-b border-line bg-raised px-3 py-1.5 font-sans text-xs text-muted" data-dnd-banner={dndNow.reason}>
        <BellOff size={13} strokeWidth={1.5} class="shrink-0" />
        <span class="min-w-0 flex-1 truncate" data-dnd-label>{dndNow.label}</span>
        {#if dndNow.reason === 'paused'}
          <button type="button" class="shrink-0 text-accent transition-colors hover:text-fg" onclick={() => void resume()} data-dnd-resume>
            Resume
          </button>
        {/if}
      </div>
      {#if resumeError}<div class="px-3 py-1 font-sans text-xs text-danger" role="alert">{resumeError}</div>{/if}
    {/if}

    <div class="flex items-center gap-2 px-3 pb-1.5 pt-2.5">
      <Tabs
        items={[
          { id: 'foryou', label: 'For you' },
          { id: 'all', label: 'All' },
        ]}
        value={tab}
        onChange={setTab}
      />
      <span class="min-w-0 flex-1 truncate text-right font-mono text-[10px] text-accent" data-bell-unread>
        {#if unreadHere}{unreadHere} unread{/if}
      </span>
      <IconButton size="sm" title="Pause notifications" active={pauseOpen} onclick={() => (pauseOpen = !pauseOpen)}>
        <BellOff size={14} strokeWidth={1.5} />
      </IconButton>
      {#if (query.data?.unread ?? 0) > 0}
        <IconButton
          size="sm"
          title="Mark all read"
          onclick={() => {
            close()
            void markRead()
          }}
        >
          <CheckCheck size={14} strokeWidth={1.5} />
        </IconButton>
      {/if}
    </div>
    {#if pauseOpen}
      <div class="border-y border-line py-1">
        <PauseMenu dnd={query.data?.dnd} onDone={() => (pauseOpen = false)} />
      </div>
    {/if}

    {#if rows.length === 0}
      <EmptyState
        variant="compact"
        title={tab === 'foryou' ? 'Nothing needs you' : 'All caught up'}
        hint={tab === 'foryou' && (query.data?.unread ?? 0) > 0 ? 'Activity in things you follow is under All.' : undefined}
      />
    {:else}
      <!-- max-h + overflow: a list that grew the page (or the dock) instead of
           scrolling would be a popover engine bug wearing a badge. -->
      <div class="max-h-[60vh] overflow-y-auto" data-bell-list={tab}>
        <NotificationList
          {rows}
          {classes}
          pictures={pictures.current}
          compact
          canArchive={v2}
          canUnread={v2}
          onOpen={(n) => open(n, close)}
          onDone={v2 ? done : undefined}
          onToggleRead={toggleRead}
        />
      </div>
    {/if}

    <div class="flex items-center border-t border-line px-2 py-1.5">
      <a
        href={p('/notifications')}
        onclick={close}
        class="flex-1 rounded-md px-1.5 py-1 text-sm text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
        data-bell-view-all
      >
        View all
      </a>
      <a
        href={p('/settings/:tab', { params: { tab: 'notifications' } })}
        onclick={close}
        class="rounded-md px-1.5 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors duration-[120ms] hover:text-fg"
      >
        Settings
      </a>
    </div>
  {/snippet}
</Popover>
