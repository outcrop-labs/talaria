<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { insecureExplanation } from '@/lib/notify-help'
  import type { BrowserAlerts } from './browser-alerts.svelte'

  // THIS BROWSER. Desktop alerts: an OS banner when Talaria is not what the
  // person is looking at. In a browser that is the Notification API plus Web
  // Push for a closed browser, and this row says which of the five states this
  // browser is in, with the one control that state allows, never a dead one:
  //
  //   default     → Turn on (the native prompt runs inside the click)
  //   granted     → On / Off (this browser's own switch), and a test banner
  //   denied      → where to unblock it: the browser's switch and the OS's
  //   insecure    → why a plain-http dev stack cannot alert, no control
  //   unsupported → that it cannot
  //
  // What the browser reports underneath (worker, subscription, the server's
  // record) is BrowserChecks, behind Troubleshooting.
  let { alerts }: { alerts: BrowserAlerts } = $props()
</script>

<!-- Rows for the This browser card. -->
<div data-notify-settings={alerts.shell ? 'shell' : alerts.perm} class="[&>*+*]:border-t [&>*+*]:border-line-subtle">
  <SettingsRow label="Desktop alerts">
    {#snippet hint()}
      {#if alerts.shell}
        Desktop app notifications are set in Settings, under Desktop.
      {:else if alerts.perm === 'insecure'}
        {insecureExplanation(location.origin)}
      {:else if alerts.perm === 'unsupported'}
        This browser does not support desktop alerts.
      {:else if alerts.perm === 'denied'}
        Blocked in {alerts.browser?.name ?? 'this browser'}.
      {:else if alerts.perm === 'default' || !alerts.on}
        Get an alert when someone needs you while Talaria is in another tab or closed.
      {:else}
        Alerts pop up while Talaria is in another tab or closed.
      {/if}
      {#if alerts.note}<span class="block">{alerts.note}</span>{/if}
    {/snippet}
    {#if !alerts.shell && alerts.perm === 'default'}
      <Button size="sm" variant="outline" onclick={alerts.turnOn}>Turn on</Button>
    {:else if !alerts.shell && alerts.perm === 'granted'}
      <Toggle checked={alerts.on} onChange={(v) => alerts.set(v)} title="Desktop alerts in this browser" />
    {/if}
  </SettingsRow>
  {#if !alerts.shell && alerts.perm === 'denied' && alerts.steps}
    <div class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">
      <ul class="list-disc space-y-1 pl-4" data-notify-steps>
        {#each [...alerts.steps.browser, ...alerts.steps.os] as step (step)}<li>{step}</li>{/each}
      </ul>
      <div class="mt-2">{alerts.steps.closing}</div>
    </div>
  {/if}
  {#if !alerts.shell && alerts.perm === 'granted' && alerts.on}
    <SettingsRow label="Test alert">
      {#snippet hint()}
        {#if alerts.testMsg}
          <span class={alerts.testMsg.ok ? undefined : 'text-danger'} data-browser-test-result>{alerts.testMsg.text}</span>
        {:else}
          Shows one banner from this browser, so you can see alerts arrive.
        {/if}
      {/snippet}
      <Button size="sm" variant="outline" onclick={() => void alerts.testHere()} data-browser-test>Send test alert</Button>
    </SettingsRow>
  {/if}
</div>
