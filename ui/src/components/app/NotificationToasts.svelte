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
  import { inDesktopShell, shellCloseNotifications } from '@/lib/desktop-shell'
  import { getJson, postJson, putJson } from '@/lib/fetch-json'
  import { lastInputAt, lookingNow, lookingWithInput, onInput } from '@/lib/looking'
  import { saveNotifyPrompt, useNotifications, type Notification, type NotificationsPayload } from '@/lib/notifications'
  import { canvasDot, startAmbient, type Ambient } from '@/lib/notify-ambient'
  import { PersistedArrivals, coalesce, present, readSince, summaryTitle, type Arrival, type SincePage } from '@/lib/notify-arrivals'
  import { idbBanneredStore, markBannered, wasBannered } from '@/lib/notify-bannered'
  import { CLAIM_WINDOW_MS, atPath, decide, selfLooking, visitReadIds, winnerIsHere, type DecideInput } from '@/lib/notify-decide'
  import { blockedSteps, detectBrowser, type BlockedSteps } from '@/lib/notify-help'
  import type { NotifyLeader } from '@/lib/notify-leader'
  import { openNotification } from '@/lib/notify-open'
  import { clientId, createAckBatcher, notePresence, presenceView, presenceWinner } from '@/lib/notify-presence'
  import { shouldAsk, turnOnResult, type PromptMemory } from '@/lib/notify-prompt'
  import { applyReadSync, registrationBanners } from '@/lib/notify-readsync'
  import { setPresence } from '@/lib/notify-status.svelte'
  import { SUMMARY_TAG, type NotificationRowV2, type NotificationsEventV2, type PresenceSnapshot } from '@/lib/notify-wire'
  import { useSession } from '@/lib/session'
  import { dismissKeyedToasts, dismissToastsByKey, pushToast, toastKeys } from '@/lib/toast.svelte'
  import { acquireNotifyRuntime, hubRole, notifyLeader, onNotifyRuntime, onUserEvent, setStreamSince } from '@/lib/user-events.svelte'
  import { navigateHref, route } from '@/router'

  // The app's notification runtime. Renders only the soft-ask card; mounted
  // in AppLayout so it runs on every surface — the toast is only honest if a
  // mention that lands while you are on Boards reaches you there.
  //
  // WHAT IT DOES WITH AN ARRIVAL. The arrival record (notify-arrivals.ts)
  // says what this browser has not yet surfaced — including what landed while
  // the page was closed or the stream was down. Against an api that serves
  // `since` it reads exactly the rows past its seq cursor (on every
  // `notification` frame, `resync`, open and poll: lossless, I-20); against
  // an older one, wave 1's rule over the newest 50. Each arrival is then
  // decided ONCE PER BROWSER, on the device the person is on
  // (notify-decide.ts, over the tab registry in notify-leader.ts and the
  // presence winner in notify-presence.ts): nothing for a feed row or during
  // Do Not Disturb, marked read when the person is looking at its page, a
  // toast in the tab they look at, nothing when they are on another device,
  // else one OS banner from the leader tab. Banners go through the service
  // worker registration — the namespace a push lands in — and the bannered
  // store, so the worker's push and the leader's banner are one alert. More
  // than three at once is one summary. A toast to a looking person and a
  // posted banner are acked (I-18), so the deferred push stays quiet.
  //
  // WHAT IT DOES WITH A READ ELSEWHERE (I-16): the banners and toasts for
  // those rows come down (notify-readsync.ts). And the tab keeps the
  // ambient signals — "(n) Talaria", a favicon dot, the app badge — on the
  // For-you count (notify-ambient.ts).
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

  /** A row as this runtime reads it: wave 1's seven keys, and the v2 ones an
   *  api since the contract adds (absent from an older one). */
  type Row = Notification & Partial<Pick<NotificationRowV2, 'seq' | 'alert' | 'class' | 'category' | 'count'>>

  const qc = useQueryClient()
  const query = useNotifications()
  const session = useSession()
  const shell = inDesktopShell()
  const bannered = idbBanneredStore()
  const loadedAt = typeof performance !== 'undefined' ? performance.timeOrigin : Date.now()
  const me = clientId()

  let perm = $state<PermissionState>(permissionState())
  let leader: NotifyLeader | null = null
  let tracker: PersistedArrivals<Row> | null = null
  let trackerFor = ''
  let hiddenArrivals = 0
  // The rows the catch-up summary on screen stands for (read-sync closes it
  // when they are all read).
  let summaryIds: string[] = []
  // The last decisions, newest last: what the diagnostics hook shows when an
  // alert did not appear (and what the status panel will read).
  const recent: Array<{ key: string; at: number; outcome: string }> = []
  const note = (key: string, outcome: string) => {
    recent.push({ key, at: Date.now(), outcome })
    if (recent.length > 50) recent.shift()
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

  const features = (): string[] => query.data?.features ?? []
  const has = (f: string) => features().includes(f)

  const markRead = (ids: string[]) => putJson('/api/notifications', { ids })
  const refetchReads = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: ['notifications'] }),
      qc.invalidateQueries({ queryKey: ['unreads'] }),
      qc.invalidateQueries({ queryKey: ['home'] }),
    ])
  const open = (n: { id: string; href?: string | null }) =>
    openNotification(n, {
      navigate: (path) => navigateHref(path),
      markRead,
      invalidate: refetchReads,
      origin: location.origin,
    })
  const openBell = () => window.dispatchEvent(new CustomEvent('talaria:open-notifications'))

  // ── The ack (I-18) ───────────────────────────────────────────────────────
  // `keepalive`: a batch still queued when the tab goes away (flushed on
  // pagehide, below) outlives the page.
  const acker = createAckBatcher((ids) => postJson('/api/notifications/ack', { ids }, { keepalive: true }))
  const ack = (ids: string[]) => {
    if (has('ack')) acker.add(ids)
  }

  // ── The runtime: the tab registry, the leader lock and the hub ───────────
  // Held by user-events.svelte.ts (the stream and the lock are one per
  // browser); this component holds the runtime for its life and decides
  // with its leader.
  $effect(() => {
    const release = acquireNotifyRuntime()
    const hook = () => {
      leader = notifyLeader()
      // A read-only diagnostics hook (the status panel and the live checks read it).
      ;(window as unknown as { __talariaNotify?: object }).__talariaNotify = {
        tabId: leader?.tabId ?? '',
        clientId: me,
        isLeader: () => notifyLeader()?.isLeader() ?? false,
        alone: leader?.alone ?? true,
        hubRole: () => hubRole(),
        presence: () => presenceView(),
        cursor: () => tracker?.cursor() ?? null,
        recent: () => recent.slice(),
      }
    }
    hook()
    const off = onNotifyRuntime(hook)
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
      visitSoon()
    }
    const onBlur = () => leader?.announce()
    const onVisible = () => {
      leader?.announce()
      askIfMissed(document.visibilityState === 'visible')
      visitSoon()
    }
    // A touch after a long still moment can make this tab LOOKING again: a
    // visit may now read what points here.
    let wasLooking = lookingWithInput()
    const offInput = onInput(() => {
      const now = lookingWithInput()
      if (now && !wasLooking) visitSoon()
      wasLooking = now
    })
    // A banner posted in the last half second is still in the ack batch: the
    // tab closing must not drop it (the phone would buzz for it).
    const flushAcks = () => void acker.flush()
    window.addEventListener('focus', onFocus)
    window.addEventListener('blur', onBlur)
    window.addEventListener('pagehide', flushAcks)
    document.addEventListener('visibilitychange', onVisible)
    const offSince = setStreamSince(() => tracker?.persistedCursor() ?? null)
    return () => {
      window.removeEventListener('focus', onFocus)
      window.removeEventListener('blur', onBlur)
      window.removeEventListener('pagehide', flushAcks)
      document.removeEventListener('visibilitychange', onVisible)
      offInput()
      offSince()
      off()
      void acker.flush()
      leader = null
      release()
    }
  })

  // ── The person's stream: arrivals, read-sync, presence ───────────────────
  $effect(() =>
    onUserEvent((ev) => {
      if (ev.type === 'notification' || ev.type === 'resync') {
        if (has('since')) void catchUp()
      } else if (ev.type === 'notifications') {
        if (typeof ev.unreadForYou === 'number') ambient?.set(ev.unreadForYou)
        void readSync(ev)
      } else if (ev.type === 'presence') {
        notePresence(ev)
        untrack(() => setPresence({ looking: ev.looking, active: ev.active, clients: ev.clients, self: me }))
      }
    }),
  )

  async function readSync(ev: NotificationsEventV2) {
    const r = await applyReadSync(ev, {
      shown: () => (shell ? Promise.resolve([]) : registrationBanners()),
      dismiss: (keys) => (keys === 'all' ? dismissKeyedToasts() : dismissToastsByKey(keys)),
      toastKeys,
      shellClose: shell ? shellCloseNotifications : undefined,
      recentTags: async () => {
        const day = Date.now() - 48 * 3600_000
        return (await bannered.entries())
          .filter(([, e]) => e.at >= day)
          .sort((a, b) => b[1].at - a[1].at)
          .slice(0, 200)
          .map(([k]) => k)
      },
      summaryIds: () => summaryIds,
      summaryClosed: () => {
        summaryIds = []
      },
      refetch: async () => {
        await qc.invalidateQueries({ queryKey: ['notifications'] })
        const d = qc.getQueryData<NotificationsPayload>(['notifications'])
        const rows = d?.notifications ?? []
        return {
          unread: new Set(rows.filter((x) => !x.readAt).map((x) => x.id)),
          read: new Set(rows.filter((x) => x.readAt).map((x) => x.id)),
          complete: rows.length < 50,
        }
      },
    })
    if (r.closed.length || r.shell.length || r.dismissed === 'all' || r.dismissed.length)
      note('read-sync', `closed ${r.closed.length}, shell ${r.shell.length}, toasts ${r.dismissed === 'all' ? 'all' : r.dismissed.length}`)
  }

  // Who the person is on, at start (frames come only when it changes; each
  // heartbeat's answer refreshes it — presence.svelte.ts).
  let presenceAsked = false
  $effect(() => {
    if (presenceAsked || !query.data || !has('presence-store')) return
    presenceAsked = true
    void getJson<PresenceSnapshot>('/api/me/presence')
      .then((p) => {
        notePresence({ looking: p.looking, active: p.active, clients: p.clients?.length ?? 0 })
        untrack(() => setPresence({ looking: p.looking, active: p.active, clients: p.clients?.length ?? 0, self: me }))
      })
      .catch(() => {})
  })

  // ── The ambient signals ──────────────────────────────────────────────────
  let ambient: Ambient | null = null
  $effect(() => {
    const a = startAmbient({
      doc: document,
      nav: shell ? null : (navigator as unknown as { setAppBadge?: (n?: number) => Promise<void> }),
      dot: canvasDot,
    })
    ambient = a
    return () => {
      ambient = null
      a.stop()
    }
  })
  $effect(() => {
    const n = query.data?.unreadForYou
    if (typeof n === 'number') untrack(() => ambient?.set(n))
  })

  // ── Opening from outside the page ────────────────────────────────────────
  $effect(() => {
    // I-1: the desktop shell's banner click. Cancelled here, so the shell
    // does not fall back to a full-page `location.assign`.
    const fromShell = (e: Event) => {
      const d = (e as CustomEvent<{ id?: string; href?: string }>).detail ?? {}
      e.preventDefault()
      // The catch-up summary names no row (its id is its tag): open the bell.
      if (d.id === SUMMARY_TAG) return openBell()
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

  // ── Visit marks read (EB-13) ─────────────────────────────────────────────
  // Arriving at a page — a route change, or the tab becoming looking — reads
  // the Direct and Following rows that point at it (a DM's conversation, a
  // finished run's page). Needs-you rows clear when they are resolved, never
  // on a glance. Only a LOOKING tab (shown, focused, touched in the last
  // minute) reads: a hidden tab never does (C8).
  let visitTimer: ReturnType<typeof setTimeout> | null = null
  const visitAsked = new Set<string>()
  function visitSoon() {
    if (visitTimer) clearTimeout(visitTimer)
    visitTimer = setTimeout(() => {
      visitTimer = null
      visit()
    }, 300)
  }
  function visit() {
    if (!lookingWithInput()) return
    const rows = (query.data?.notifications ?? []) as Row[]
    const ids = visitReadIds(rows, location.pathname, location.origin).filter((id) => !visitAsked.has(id))
    if (!ids.length) return
    for (const id of ids) visitAsked.add(id)
    note(ids.join(','), `read: visited ${location.pathname}`)
    void markRead(ids)
      .then(refetchReads)
      .catch(() => {
        for (const id of ids) visitAsked.delete(id)
      })
  }
  $effect(() => {
    void route.pathname
    untrack(() => visitSoon())
  })
  $effect(() => {
    // The list moved (a new row may point here).
    void query.data
    untrack(() => visitSoon())
  })

  // ── Arrivals ─────────────────────────────────────────────────────────────
  // Rows the record has not surfaced. The arrival is decided per row; the
  // re-alert (a DM row refreshed by a newer message) re-shows under its tag.
  $effect(() => {
    const data = query.data
    const rows = data?.notifications as Row[] | undefined
    const uid = session.data?.id
    if (!rows || !uid) return
    untrack(() => {
      if (!tracker || trackerFor !== uid) {
        tracker = new PersistedArrivals<Row>(uid)
        trackerFor = uid
      }
      if (data?.features?.includes('since')) {
        // The list's snapshot seq says whether anything committed past the
        // cursor: only then is a `?since` read worth making (a frame already
        // asked for one).
        const c = tracker.cursor()
        if (!tracker.hasRecord() || c === null || (typeof data.seq === 'number' && data.seq > c)) void catchUp()
        return
      }
      void surface(tracker.arrive(rows))
    })
  })

  /** The lossless catch-up (I-20): every row past the cursor, coalesced so a
   *  burst of frames is one read and one more. */
  const catchUp = coalesce(async () => {
    const t = tracker
    if (!t) return
    if (!t.hasRecord()) {
      // A browser with no record for this person: a silent baseline, its
      // cursor at the list's snapshot (a fresh profile must not replay the
      // inbox as a burst).
      const d = qc.getQueryData<NotificationsPayload>(['notifications'])
      if (!d) return
      t.arrive(d.notifications as Row[], Date.now(), typeof d.seq === 'number' ? d.seq : null)
      return
    }
    const from = t.cursor() ?? 0
    const r = await readSince<Row>((since) => getJson<SincePage<Row>>(`/api/notifications?since=${since}`), from)
    if (tracker !== t) return
    await surface(t.arrive(r.rows, Date.now(), r.through))
    if (!r.complete && r.through !== null) void catchUp()
  })

  /** The decision inputs every rule shares, read now. */
  function context(): Omit<DecideInput, 'read' | 'alert' | 'breaksThrough' | 'atRowPath'> {
    const l = leader
    const visible = document.visibilityState === 'visible'
    let focused = false
    try {
      focused = document.hasFocus()
    } catch {
      focused = false
    }
    const d = query.data
    return {
      self: { visible, focused, tabId: l?.tabId, lastInputAt: Math.max(lastInputAt(), l?.lastInputAt() ?? 0), clientId: me },
      others: l?.others() ?? [],
      isLeader: l?.isLeader() ?? true,
      bannersOn: browserNotifyEnabled(),
      dnd: d?.dnd ? { active: !!d.dnd.active } : null,
      winner: has('presence-store') ? presenceWinner() : null,
      now: Date.now(),
    }
  }

  /** Does this row's class break through Do Not Disturb (I-14)? */
  function breaksThrough(row: Row): boolean {
    const d = query.data
    if (d?.dnd && d.dnd.allowNeedsYou === false) return false
    const cls = d?.classes?.find((c) => c.id === row.class)
    if (cls) return cls.breaksThrough
    return row.category === 'needs_you'
  }

  async function surface(all: Arrival<Row>[]) {
    // A version this browser already showed as an OS banner — the worker's
    // push while every tab was closed, say — is not surfaced again: one alert
    // per notification per browser, and a catch-up summary must not stack a
    // banner on top of the worker's per-row ones. The bell still lists them.
    // Then the rules that need no other tab: a feed row and a row held by Do
    // Not Disturb are recorded only; a row whose page the person is looking
    // at is read on sight.
    const arrivals: Arrival<Row>[] = []
    const seen: string[] = []
    const ctx = context()
    for (const a of all) {
      if (await wasBannered(bannered, a.row.id, a.row.createdAt)) {
        note(a.row.id, 'none: already bannered')
        continue
      }
      const visitable = a.row.category === 'direct' || a.row.category === 'following'
      const d = decide({
        ...ctx,
        read: !!a.row.readAt,
        alert: a.row.alert ?? null,
        breaksThrough: breaksThrough(a.row),
        atRowPath: visitable && atPath(a.row.href, location.pathname, location.origin),
      })
      if (d === 'seen') {
        seen.push(a.row.id)
        note(a.row.id, 'none: read on sight (looking at its page)')
      } else if (d === 'none' && (a.row.readAt || a.row.alert === 'feed' || (ctx.dnd?.active && !breaksThrough(a.row)))) {
        note(a.row.id, a.row.readAt ? 'none: read' : a.row.alert === 'feed' ? 'none: feed' : 'none: do not disturb')
      } else arrivals.push(a)
    }
    if (seen.length) {
      for (const id of seen) visitAsked.add(id)
      void markRead(seen).then(refetchReads).catch(() => {})
    }
    const p = present(arrivals)
    if (!p) return
    if (!lookingHere()) hiddenArrivals += arrivals.length
    if (p.kind === 'summary') {
      await surfaceOne({
        key: SUMMARY_TAG,
        id: '',
        ids: arrivals.map((a) => a.row.id),
        title: summaryTitle(p.count, p.newest.title),
        body: undefined,
        href: '',
        kind: 'summary',
        version: p.newest.createdAt,
        realert: true,
        summary: true,
      })
      return
    }
    for (const a of p.items) {
      await surfaceOne({
        key: a.row.id,
        id: a.row.id,
        ids: [a.row.id],
        title: a.row.title,
        body: a.row.body || undefined,
        href: a.row.href || '',
        kind: a.row.kind,
        version: a.row.createdAt,
        realert: a.realert,
        summary: false,
      })
    }
  }

  async function surfaceOne(n: {
    key: string
    id: string
    ids: string[]
    title: string
    body?: string
    href: string
    kind: string
    version: string
    realert: boolean
    summary: boolean
  }) {
    const l = leader
    const ctx = context()
    // Read, feed and Do Not Disturb were settled per row before this (a
    // summary stands for rows that each passed them): what is left is which
    // tab, and which device.
    const decision = decide({ ...ctx, dnd: null, read: false })
    const toast = () =>
      pushToast({
        key: n.key,
        title: n.title,
        body: n.body,
        href: n.href || undefined,
        onOpen: n.summary ? openBell : () => void open({ id: n.id, href: n.href }),
      })
    if (decision === 'toast') {
      toast()
      l?.claim(n.key, n.version)
      // Shown to a person looking at it: the deferred push need not follow.
      if (selfLooking(ctx.self, ctx.now)) ack(n.ids)
      // In front of the person: a reason to ask now. A toast in a visible
      // window they are not typing in asks when they come to it (focus).
      if (lookingHere()) askTrigger++
      note(n.key, ctx.self.focused ? 'toast' : 'toast: banners off, the visible tab touched last')
      return
    }
    if (decision !== 'banner') {
      if (!winnerIsHere(ctx)) return note(n.key, `none: the person is on another device (${ctx.winner})`)
      return note(n.key, ctx.bannersOn ? 'none: another tab has it' : 'none: banners off, another tab shows it')
    }
    if (l && (await l.waitForClaim(n.key, n.version, CLAIM_WINDOW_MS))) return note(n.key, 'none: claimed by a tab')
    if (await wasBannered(bannered, n.key, n.version)) return note(n.key, 'none: already bannered')
    if (!n.realert && (await onScreen(n.key))) return note(n.key, 'none: already on screen')
    await markBannered(bannered, n.key, n.version)
    note(n.key, n.realert ? 'banner (re-alert)' : 'banner')
    if (n.summary) summaryIds = n.ids
    const shown = await showOsNotification(
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
    if (shown) ack(n.ids)
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
