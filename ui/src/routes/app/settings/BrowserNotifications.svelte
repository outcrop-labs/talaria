<script lang="ts">
  import Segmented from '@/components/ui/Segmented.svelte'
  import {
    browserNotifyEnabled,
    permissionState,
    requestBrowserNotify,
    setBrowserNotifyPref,
    type PermissionState,
  } from '@/lib/browser-notify'

  // Desktop notifications: the browser tapping you on the shoulder when
  // Talaria is in another tab or window. Purely a client concern — the
  // permission lives with the browser, so nothing in this row round-trips
  // the server. It sits inside the notifications section because it answers
  // the same question the routing table does ("where does this reach me"),
  // for the one destination the server can't see.
  const OPTIONS = [
    { id: 'on' as const, label: 'On' },
    { id: 'off' as const, label: 'Off' },
  ]

  let perm = $state<PermissionState>(permissionState())
  let on = $state(browserNotifyEnabled())

  const set = async (wanted: boolean) => {
    if (wanted) {
      // The click IS the gesture the permission prompt requires
      // (browser-notify.ts, rule 1); asking outside one is how a site ends
      // up reflex-denied forever.
      perm = await requestBrowserNotify()
      if (perm === 'granted') setBrowserNotifyPref(true)
    } else {
      // Browsers give no way to hand a grant back from a page; our own pref
      // is what makes "off" mean off.
      setBrowserNotifyPref(false)
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
          On. A browser notification pops up when something new lands while Talaria is in another tab or window. The in-app toast always shows.
        {:else}
          Off. Turn on to also get a browser notification when something lands while you're elsewhere.
        {/if}
      </div>
    </div>
    {#if perm !== 'unsupported'}
      <Segmented options={OPTIONS} value={on ? 'on' : 'off'} onChange={(v) => void set(v === 'on')} />
    {/if}
  </div>
</div>
