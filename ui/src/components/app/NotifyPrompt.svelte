<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import { fade, fly, QUICK } from '@/lib/motion'
  import { intoToastLead } from '@/lib/toast.svelte'
  import type { BlockedSteps } from '@/lib/notify-help'

  // The one-time invitation to turn desktop alerts on — the card, not the
  // decision. WHEN it shows is notify-prompt.ts's `shouldAsk`, decided by
  // NotificationToasts (never on load: an arrival just toasted here, or came
  // in while the tab was hidden, and the person has touched the page).
  //
  // It sits at the top of the toast stack, so the toast that gave it a reason
  // stays readable below it. The native browser prompt runs inside the Turn
  // on click (browser-notify.ts, rule 1); the card's answer is remembered on
  // the account, spaced 7 days then 30, and retired after the third "Not now".

  interface Props {
    show: boolean
    /** A line under the ask: the native prompt closed without an answer. */
    message?: string | null
    /** The browser blocked it: the steps to undo that, instead of the ask. */
    blocked?: BlockedSteps | null
    browserName?: string
    onTurnOn: () => void
    onNotNow: () => void
    onClose: () => void
  }
  let { show, message = null, blocked = null, browserName = 'your browser', onTurnOn, onNotNow, onClose }: Props = $props()
</script>

{#if show}
  <div
    use:intoToastLead
    in:fly={{ y: 8, duration: 220 }}
    out:fade={QUICK}
    class="pointer-events-auto"
    role="dialog"
    aria-label="Turn on desktop alerts"
    data-notify-prompt
  >
    <Panel class="p-4 shadow-[var(--theme-shadow-2)]">
      {#if blocked}
        <div class="font-sans text-sm font-medium text-fg">Desktop alerts are blocked in {browserName}</div>
        <ul class="mt-2 list-disc space-y-1 pl-4 font-sans text-xs text-muted">
          {#each [...blocked.browser, ...blocked.os] as step (step)}<li>{step}</li>{/each}
        </ul>
        <div class="mt-2 font-sans text-xs text-muted">{blocked.closing}</div>
        <div class="mt-3 flex justify-end">
          <Button variant="ghost" size="sm" onclick={onClose}>Close</Button>
        </div>
      {:else}
        <div class="font-sans text-sm text-fg">
          Get a desktop alert when someone needs you — mentions, DMs, approvals.
        </div>
        {#if message}<div class="mt-1.5 font-sans text-xs text-muted">{message}</div>{/if}
        <div class="mt-3 flex justify-end gap-2">
          <Button variant="ghost" size="sm" onclick={onNotNow}>Not now</Button>
          <Button size="sm" onclick={onTurnOn}>Turn on</Button>
        </div>
      {/if}
    </Panel>
  </div>
{/if}
