<script lang="ts">
  import { Bell, BellOff, BellRing } from '@lucide/svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
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
  import { relativeTime } from '@/lib/fleet'
  import { saveNotifyPrompt, useMarkNotificationsRead, useNotifications } from '@/lib/notifications'
  import { blockedSteps, detectBrowser, insecureExplanation } from '@/lib/notify-help'
  import { turnOnResult } from '@/lib/notify-prompt'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'

  // The dock's bell: the notification inbox as a dropdown beside the account
  // chip. Clicking a row is "take me there AND clear it" in one motion — the
  // mark-read that this bell finally wires to a real click is what lets an
  // unread row ever leave the count, the brief, and (once push lands) the OS.
  //
  // The count follows the rail's `/home` doctrine: a failed read is `!`, never
  // a silent 0 — the bell is the one surface a person glances at to ask "is
  // anything waiting", and an outage must not answer "no".
  const query = useNotifications()
  const markRead = useMarkNotificationsRead()

  const unread: number | null = $derived(
    query.isError && query.data === undefined ? null : (query.data?.unread ?? 0),
  )
  const unreadLabel = $derived(unread === null ? '!' : unread > 99 ? '99+' : String(unread))
  const unreadTitle = $derived(
    unread === null ? 'Could not load your notifications' : undefined,
  )
  const rows = $derived(query.data?.notifications ?? [])

  // THE DESKTOP-ALERTS ROW, at the top of the popover: the one place that
  // always says what this browser will do and how to change it — the soft
  // ask's card retires after three "Not now"s, this row never does. Never in
  // the desktop shell: the shell asks for itself (Settings → Desktop).
  const qc = useQueryClient()
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

  // A summary toast or banner ("5 new — …") opens the bell itself.
  let popoverOpen = $state(false)
  $effect(() => {
    const show = () => (popoverOpen = true)
    window.addEventListener('talaria:open-notifications', show)
    return () => window.removeEventListener('talaria:open-notifications', show)
  })

  // A row click clears its row and goes where it points, in that order — the
  // navigation must not wait on the PUT, and a notification with no href
  // (board-access outcomes) still deserves its click-to-clear.
  function open(n: { id: string; href: string }, close: () => void) {
    close()
    void markRead({ ids: [n.id] })
    if (n.href) void navigateHref(n.href)
  }
</script>

<Popover align="right" offset={8} class="w-80" bind:open={popoverOpen}>
  {#snippet trigger(open)}
    <button
      type="button"
      aria-expanded={open}
      aria-label="Notifications"
      class="relative flex h-9 w-9 items-center justify-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
    >
      <Bell size={16} strokeWidth={1.5} class="shrink-0" />
      {#if unread === null || unread > 0}
        <span
          title={unreadTitle}
          class="absolute right-0 top-0 font-mono text-[10px] font-medium leading-3 tracking-[0.05em] {unread === null ? 'text-[color:var(--theme-danger)]' : 'text-accent'}"
        >
          {unreadLabel}
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
    <div class="flex items-baseline justify-between px-3 pb-1.5 pt-2.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Notifications</span>
      {#if unread}
        <span class="font-mono text-[10px] text-accent">{unread} unread</span>
      {/if}
    </div>

    {#if rows.length === 0}
      <div class="px-3 py-6 text-center text-sm text-muted">All caught up.</div>
    {:else}
      <!-- max-h + overflow: the dropdown lists the last 50; a list that grew
           the page (or the dock) instead of scrolling would be a popover
           engine bug wearing a notifications badge. -->
      <div class="max-h-80 overflow-y-auto">
        {#each rows as n (n.id)}
          <button
            type="button"
            onclick={() => open(n, close)}
            class="flex w-full items-start gap-2.5 border-b border-line px-3 py-2 text-left transition-colors duration-[120ms] dither-fill last:border-b-0"
          >
            <!-- The dot is the read state: accent while the row waits, absent
                 once it has been seen. Read rows dim to muted so the eye
                 finds what is NEW, not what is merely recent. -->
            {#if !n.readAt}
              <StatusDot status="accent" class="mt-[7px]" />
            {:else}
              <span class="mt-[7px] h-1.5 w-1.5 shrink-0"></span>
            {/if}
            <span class="min-w-0 flex-1">
              <span class="block truncate text-sm {n.readAt ? 'text-muted' : 'font-medium text-fg'}">{n.title}</span>
              {#if n.body}
                <span class="mt-0.5 block truncate text-xs {n.readAt ? 'text-ink-dim' : 'text-muted'}">{n.body}</span>
              {/if}
              <span class="mt-0.5 block font-mono text-[10px] text-ink-dim">{relativeTime(n.createdAt)}</span>
            </span>
          </button>
        {/each}
      </div>
    {/if}

    {#if unread}
      <div class="border-t border-line px-2 py-1.5">
        <button
          type="button"
          onclick={() => {
            close()
            void markRead()
          }}
          class="flex w-full items-center rounded-md px-1.5 py-1 text-left text-sm text-muted transition-colors duration-[120ms] dither-fill"
        >
          Mark all read
        </button>
      </div>
    {/if}
  {/snippet}
</Popover>
