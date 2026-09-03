<script lang="ts">
  import Segmented from '@/components/ui/Segmented.svelte'
  import {
    browserNotifyEnabled,
    ensurePushSubscription,
    permissionState,
    requestBrowserNotify,
    setBrowserNotifyPref,
    stopPushSubscription,
    type PermissionState,
    type PushOutcome,
  } from '@/lib/browser-notify'

  // Desktop notifications: the browser tapping you on the shoulder when
  // Talaria is in another tab or window — and, through Web Push, when it is
  // closed entirely. The permission lives with the browser, but the
  // closed-tab half is a subscription this row files with the server
  // (browser-notify.ts), so turning it on and off both round-trip once. It
  // sits inside the notifications section because it answers the same
  // question the routing table does ("where does this reach me"), for the
  // one destination the routing table can't name per-device.
  const OPTIONS = [
    { id: 'on' as const, label: 'On' },
    { id: 'off' as const, label: 'Off' },
  ]

  let perm = $state<PermissionState>(permissionState())
  let on = $state(browserNotifyEnabled())
  let push = $state<PushOutcome | null>(null)

  const set = async (wanted: boolean) => {
    if (wanted) {
      // The click IS the gesture the permission prompt requires
      // (browser-notify.ts, rule 1); asking outside one is how a site ends
      // up reflex-denied forever. The same gesture files the push
      // subscription — closed-tab reach rides the same grant.
      perm = await requestBrowserNotify()
      if (perm === 'granted') {
        setBrowserNotifyPref(true)
        push = await ensurePushSubscription()
      }
    } else {
      // Browsers give no way to hand a grant back from a page; our own pref
      // is what makes "off" mean off — for the tab's copies AND the closed
      // browser's, whose subscription this retires at both ends.
      setBrowserNotifyPref(false)
      push = null
      void stopPushSubscription()
    }
    on = browserNotifyEnabled()
  }
</script>

<div class="mt-5 border-t border-line-subtle pt-4">
  <div class="flex items-center gap-4">
    <div class="min-w-0 flex-1">
      <div class="text-sm text-fg">Desktop notifications</div>
      <div class="font-sans text-xs text-muted">
        {#if perm === 'denied'}
          Your browser is blocking notifications from this site. Allow them in the browser's site settings, then turn this on.
        {:else if perm === 'unsupported'}
          This browser doesn't support desktop notifications.
        {:else if on}
          On. A browser notification pops up when something new lands while Talaria is in another tab or window — and, if this
          browser took the push subscription, when it's closed entirely. The in-app toast always shows.
        {:else}
          Off. Turn on to also get a browser notification when something lands while you're elsewhere — closed browsers included.
        {/if}
        {#if on && push === 'failed'}
          Push didn't register for closed browsers this time (open-tab notifications are unaffected) — toggling off and on retries.
        {/if}
      </div>
    </div>
    {#if perm !== 'unsupported'}
      <Segmented options={OPTIONS} value={on ? 'on' : 'off'} onChange={(v) => void set(v === 'on')} />
    {/if}
  </div>
</div>
