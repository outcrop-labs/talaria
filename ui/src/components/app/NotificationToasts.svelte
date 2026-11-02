<script lang="ts">
  import { untrack } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import NotifyPrompt from '@/components/app/NotifyPrompt.svelte'
  import {
    browserNotifyEnabled,
    browserNotifyPrefOff,
    ensurePushSubscription,
    permissionState,
    showOsNotification,
    turnOnDesktopAlerts,
    watchPermission,
    type PermissionState,
  } from '@/lib/browser-notify'
  import { inDesktopShell } from '@/lib/desktop-shell'
  import { putJson } from '@/lib/fetch-json'
  import { saveNotifyPrompt, useNotifications, type Notification, type NotificationsPayload } from '@/lib/notifications'
  import { PersistedArrivals, present, summaryTitle, type Arrival } from '@/lib/notify-arrivals'
  import { idbBanneredStore, markBannered, wasBannered } from '@/lib/notify-bannered'
  import { CLAIM_WINDOW_MS, decide } from '@/lib/notify-decide'
  import { blockedSteps, detectBrowser, type BlockedSteps } from '@/lib/notify-help'
  import { lookingNow } from '@/lib/looking'
  import { browserLeaderEdges, startNotifyLeader, type NotifyLeader } from '@/lib/notify-leader'
  import { openNotification } from '@/lib/notify-open'
  import { shouldAsk, turnOnResult, type PromptMemory } from '@/lib/notify-prompt'
  import { useSession } from '@/lib/session'
  import { pushToast } from '@/lib/toast.svelte'
  import { navigateHref } from '@/router'

  // The app's notification runtime. Renders only the soft-ask card; mounted
  // in AppLayout so it runs on every surface — the toast is only honest if a
  // mention that lands while you are on Boards reaches you there.
  //
  // WHAT IT DOES WITH AN ARRIVAL. The inbox query (polled every 30 s, hidden
  // tabs included, and invalidated by the firehose on every notification
  // event and every stream reopen) hands each fresh page to the arrival
  // record (notify-arrivals.ts), which says what this browser has not yet
  // surfaced — including what landed while the page was closed or the
  // stream was down. Each arrival is then decided ONCE PER BROWSER
  // (notify-decide.ts over the tab registry in notify-leader.ts): a toast in
  // the tab the person is looking at, else one OS banner from the leader tab,
  // else nothing here. Banners go through the service worker registration —
  // the namespace a push lands in — and the bannered store, so the worker's
  // push and the leader's banner are one alert. More than three at once is
  // one summary.
  //
  // WHAT IT DOES WITH A CLICK. Every way a notification is answered — a
  // toast, an OS banner relayed by the worker, the desktop shell's
  // `talaria:notification-open` event, the `new Notification` fallback —
  // goes through notify-open.ts: route in place, then mark the row read.
  //
  // AND THE SUBSCRIPTION. A browser whose grant already stands files its push
  // subscription at boot and again when the permission turns granted — no
  // click needed — and the soft ask (notify-prompt.ts) invites the grant at
  // a moment that has a reason.
  const qc = useQueryClient()
  const query = useNotifications()
  const session = useSession()
  const shell = inDesktopShell()
  const bannered = idbBanneredStore()
  const loadedAt = typeof performance !== 'undefined' ? performance.timeOrigin : Date.now()

  let perm = $state<PermissionState>(permissionState())
  let leader: NotifyLeader | null = null
  let tracker: PersistedArrivals<Notification> | null = null
  let trackerFor = ''
  let hiddenArrivals = 0
  // The last decisions, newest last: what the diagnostics hook shows when an
  // alert did not appear (and what the status panel will read).
  const recent: Array<{ key: string; at: number; outcome: string }> = []
  const note = (key: string, outcome: string) => {
    recent.push({ key, at: Date.now(), outcome })
    if (recent.length > 20) recent.shift()
  }

  // Soft-ask state.
  let askTrigger = $state(0)
  let promptShown = $state(false)
  // Answered on this page (Not now, or turned on): the card does not come
  // back before the page is reloaded, whatever a refetch races in with —
  // or whether the PATCH that records the answer landed at all.
  let promptAnswered = false
  let promptMessage = $state<string | null>(null)
  let promptBlocked = $state<BlockedSteps | null>(null)
  const browser = typeof navigator !== 'undefined' ? detectBrowser(navigator.userAgent, { brave: 'brave' in navigator }) : null

  // The one "looking" rule, shared with ChannelView's read cursor: a page the
  // person looks at toasts, and only it reads a conversation.
  const lookingHere = () => lookingNow()

  const open = (n: { id: string; href?: string | null }) =>
    openNotification(n, {
      navigate: (path) => navigateHref(path),
      markRead: (ids) => putJson('/api/notifications', { ids }),
      invalidate: () =>
        Promise.all([
          qc.invalidateQueries({ queryKey: ['notifications'] }),
          qc.invalidateQueries({ queryKey: ['unreads'] }),
          qc.invalidateQueries({ queryKey: ['home'] }),
        ]),
      origin: location.origin,
    })
  const openBell = () => window.dispatchEvent(new CustomEvent('talaria:open-notifications'))
  /** The catch-up summary's tag — one OS copy, replaced by the next. */
  const SUMMARY_KEY = 'talaria-summary'

  // ── The tab registry and the leader lock ─────────────────────────────────
  $effect(() => {
    const start = () => {
      const l = startNotifyLeader({
        ...browserLeaderEdges(),
        self: () => ({ visible: document.visibilityState === 'visible', focused: document.hasFocus() }),
      })
      leader = l
      // A read-only diagnostics hook (the status panel and the live checks read it).
      ;(window as unknown as { __talariaNotify?: object }).__talariaNotify = {
        tabId: l.tabId,
        isLeader: () => leader?.isLeader() ?? false,
        alone: l.alone,
        recent: () => recent.slice(),
      }
    }
    start()
    // Arrivals landed while the person was not looking at this tab (hidden,
    // or visible on another monitor while they typed elsewhere): coming back
    // to it is a reason to ask.
    const askIfMissed = (back: boolean) => {
      if (back && hiddenArrivals > 0) {
        hiddenArrivals = 0
        askTrigger++
      }
    }
    const onFocus = () => {
      leader?.announce()
      askIfMissed(lookingHere())
    }
    const onBlur = () => leader?.announce()
    const input = () => leader?.noteInput()
    const onVisible = () => {
      leader?.announce()
      askIfMissed(document.visibilityState === 'visible')
    }
    // Leaving: release the lock and say goodbye, so the next tab leads now.
    // A page restored from the back/forward cache comes back with that
    // runtime closed — it starts a new one (a new tab id, a new place in the
    // lock's queue) instead of deciding with no registry.
    const bye = () => {
      leader?.close()
      leader = null
    }
    const back = (e: PageTransitionEvent) => {
      if (e.persisted && !leader) start()
    }
    window.addEventListener('focus', onFocus)
    window.addEventListener('blur', onBlur)
    document.addEventListener('visibilitychange', onVisible)
    window.addEventListener('pointerdown', input, { capture: true })
    window.addEventListener('keydown', input, { capture: true })
    window.addEventListener('pagehide', bye)
    window.addEventListener('pageshow', back)
    return () => {
      window.removeEventListener('focus', onFocus)
      window.removeEventListener('blur', onBlur)
      document.removeEventListener('visibilitychange', onVisible)
      window.removeEventListener('pointerdown', input, { capture: true })
      window.removeEventListener('keydown', input, { capture: true })
      window.removeEventListener('pagehide', bye)
      window.removeEventListener('pageshow', back)
      bye()
    }
  })

  // ── Opening from outside the page ────────────────────────────────────────
  $effect(() => {
    // I-1: the desktop shell's banner click. Cancelled here, so the shell
    // does not fall back to a full-page `location.assign`.
    const fromShell = (e: Event) => {
      const d = (e as CustomEvent<{ id?: string; href?: string }>).detail ?? {}
      e.preventDefault()
      // The catch-up summary names no row (its id is its tag): open the bell.
      if (d.id === SUMMARY_KEY) return openBell()
      void open({ id: typeof d.id === 'string' ? d.id : '', href: d.href })
    }
    window.addEventListener('talaria:notification-open', fromShell)
    // The service worker relaying an OS banner click: answer at once (the
    // worker navigates the page itself if no answer comes within a second),
    // then route in place.
    const container = 'serviceWorker' in navigator ? navigator.serviceWorker : null
    const fromWorker = (e: MessageEvent) => {
      const m = e.data as { type?: string; id?: string; href?: string; summary?: boolean } | null
      if (!m || m.type !== 'talaria:notification-open') return
      e.ports?.[0]?.postMessage({ type: 'talaria:notification-opened', id: m.id ?? '' })
      if (m.summary) openBell()
      else void open({ id: m.id ?? '', href: m.href })
    }
    container?.addEventListener('message', fromWorker)
    container?.startMessages?.()
    return () => {
      window.removeEventListener('talaria:notification-open', fromShell)
      container?.removeEventListener('message', fromWorker)
    }
  })

  // ── The subscription, from boot ──────────────────────────────────────────
  $effect(() => {
    const heal = (p: PermissionState) => {
      if (!shell && p === 'granted' && !browserNotifyPrefOff()) void ensurePushSubscription()
    }
    heal(perm)
    return watchPermission((p) => {
      perm = p
      heal(p)
    })
  })

  // ── Arrivals ─────────────────────────────────────────────────────────────
  // Rows the record has not surfaced. The arrival is decided per row; the
  // re-alert (a DM row refreshed by a newer message) re-shows under its tag.
  $effect(() => {
    const rows = query.data?.notifications
    const uid = session.data?.id
    if (!rows || !uid) return
    if (!tracker || trackerFor !== uid) {
      tracker = new PersistedArrivals<Notification>(uid)
      trackerFor = uid
    }
    void surface(tracker.arrive(rows))
  })

  async function surface(all: Arrival<Notification>[]) {
    // A version this browser already showed as an OS banner — the worker's
    // push while every tab was closed, say — is not surfaced again: one alert
    // per notification per browser, and a catch-up summary must not stack a
    // banner on top of the worker's per-row ones. The bell still lists them.
    const arrivals: Arrival<Notification>[] = []
    for (const a of all) {
      if (await wasBannered(bannered, a.row.id, a.row.createdAt)) note(a.row.id, 'none: already bannered')
      else arrivals.push(a)
    }
    const p = present(arrivals)
    if (!p) return
    if (!lookingHere()) hiddenArrivals += arrivals.length
    if (p.kind === 'summary') {
      await surfaceOne({
        key: SUMMARY_KEY,
        id: '',
        title: summaryTitle(p.count, p.newest.title),
        body: undefined,
        href: '',
        kind: 'summary',
        version: p.newest.createdAt,
        read: false,
        realert: true,
        summary: true,
      })
      return
    }
    for (const a of p.items) {
      await surfaceOne({
        key: a.row.id,
        id: a.row.id,
        title: a.row.title,
        body: a.row.body || undefined,
        href: a.row.href || '',
        kind: a.row.kind,
        version: a.row.createdAt,
        read: !!a.row.readAt,
        realert: a.realert,
        summary: false,
      })
    }
  }

  async function surfaceOne(n: {
    key: string
    id: string
    title: string
    body?: string
    href: string
    kind: string
    version: string
    read: boolean
    realert: boolean
    summary: boolean
  }) {
    const l = leader
    const self = {
      visible: document.visibilityState === 'visible',
      focused: document.hasFocus(),
      tabId: l?.tabId,
      lastInputAt: l?.lastInputAt() ?? 0,
    }
    const bannersOn = browserNotifyEnabled()
    const decision = decide({
      read: n.read,
      self,
      others: l?.others() ?? [],
      isLeader: l?.isLeader() ?? true,
      bannersOn,
      now: Date.now(),
    })
    const toast = () =>
      pushToast({
        title: n.title,
        body: n.body,
        href: n.href || undefined,
        onOpen: n.summary ? openBell : () => void open({ id: n.id, href: n.href }),
      })
    if (decision === 'toast') {
      toast()
      l?.claim(n.key, n.version)
      // In front of the person: a reason to ask now. A toast in a visible
      // window they are not typing in asks when they come to it (focus).
      if (lookingHere()) askTrigger++
      note(n.key, self.focused ? 'toast' : 'toast: banners off, the visible tab touched last')
      return
    }
    if (decision !== 'banner') {
      return note(n.key, bannersOn ? 'none: another tab has it' : 'none: banners off, another tab shows it')
    }
    if (l && (await l.waitForClaim(n.key, n.version, CLAIM_WINDOW_MS))) return note(n.key, 'none: claimed by a tab')
    if (await wasBannered(bannered, n.key, n.version)) return note(n.key, 'none: already bannered')
    if (!n.realert && (await onScreen(n.key))) return note(n.key, 'none: already on screen')
    await markBannered(bannered, n.key, n.version)
    note(n.key, n.realert ? 'banner (re-alert)' : 'banner')
    await showOsNotification(
      {
        id: n.id,
        tag: n.key,
        title: n.title,
        body: n.body,
        href: n.href,
        kind: n.kind,
        renotify: n.realert,
        createdAt: n.version,
        summary: n.summary,
      },
      (o) => void open(o),
    )
  }

  /** Is a banner with this tag already on screen (from the worker, say)? */
  async function onScreen(tag: string): Promise<boolean> {
    if (shell || !('serviceWorker' in navigator)) return false
    try {
      const reg = await navigator.serviceWorker.getRegistration()
      return !!reg && (await reg.getNotifications({ tag })).length > 0
    } catch {
      return false
    }
  }

  // ── The soft ask ─────────────────────────────────────────────────────────
  // Decided at the moment a reason fires, and only then: each trigger is
  // consumed once, so a later poll can never raise the card on its own.
  let handledTrigger = 0
  $effect(() => {
    const t = askTrigger
    untrack(() => {
      if (t === handledTrigger) return
      handledTrigger = t
      if (promptShown || promptAnswered) return
      const ask = shouldAsk({
        permission: perm,
        inShell: shell,
        memory: query.data?.prompt,
        now: Date.now(),
        loadedAt,
        hasBeenActive: navigator.userActivation?.hasBeenActive ?? true,
        trigger: true,
      })
      if (ask) {
        promptMessage = null
        promptBlocked = null
        promptShown = true
      }
    })
  })

  function turnOn() {
    // turnOnDesktopAlerts asks the browser synchronously, inside this click.
    void turnOnDesktopAlerts().then(async (after) => {
      perm = after
      const r = turnOnResult(after)
      if (r.kind === 'on') {
        promptShown = false
        promptAnswered = true
        rememberPrompt(await saveNotifyPrompt('granted'))
      } else if (r.kind === 'closed' || r.kind === 'unavailable') {
        promptMessage = r.message
      } else if (browser) {
        promptBlocked = blockedSteps(browser)
      } else {
        promptShown = false
      }
    })
  }

  async function notNow() {
    promptShown = false
    promptAnswered = true
    rememberPrompt(await saveNotifyPrompt('dismissed'))
  }

  /** The PATCH's own answer goes straight into the cache, so the next ask
   *  reads the new memory without waiting on a refetch; then refetch. */
  function rememberPrompt(memory: PromptMemory | null) {
    if (memory) {
      qc.setQueryData<NotificationsPayload>(['notifications'], (old) => (old ? { ...old, prompt: memory } : old))
    }
    void qc.invalidateQueries({ queryKey: ['notifications'] })
  }
</script>

<NotifyPrompt
  show={promptShown}
  message={promptMessage}
  blocked={promptBlocked}
  browserName={browser?.name}
  onTurnOn={turnOn}
  onNotNow={() => void notNow()}
  onClose={() => (promptShown = false)}
/>
