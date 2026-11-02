<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import { inDesktopShell, shellNotificationStatus, shellRequestNotificationPermission } from '@/lib/desktop-shell'
  import { fade, fly, QUICK } from '@/lib/motion'
  import { MIN_LOADED_MS, SHELL_ASK_KEY, createShellAsk, type ShellAskView } from '@/lib/notify-prompt'
  import { readText, writeText } from '@/lib/persist'

  // The desktop shell's notification ask, INSIDE an instance — the card, not
  // the decision (notify-prompt.ts `createShellAsk`).
  //
  // macOS refuses every banner until the person has allowed Talaria, and it
  // asks only when the app requests it — which the shell does from a click.
  // The launcher's card and Settings → Desktop were the only clicks, and a
  // person who resumes straight into an instance (every 0.11 upgrader) never
  // saw either: no banners at all, and nothing said why. This card shows
  // while the shell reports `not-determined` (macOS alone does), once the
  // page has been open a while and touched; its button is the click the OS
  // prompt runs in. "Not now" is remembered for this instance, and Settings →
  // Desktop keeps the button.
  //
  // It sits in the toast stack's lead slot, like the browser's card
  // (NotifyPrompt.svelte, which never shows in the shell).

  let view = $state<ShellAskView>({ kind: 'hidden' })
  const loadedAt = typeof performance !== 'undefined' ? performance.timeOrigin : Date.now()

  const ask = createShellAsk({
    inShell: inDesktopShell,
    status: shellNotificationStatus,
    request: shellRequestNotificationPermission,
    memory: { get: () => readText(SHELL_ASK_KEY), set: (v) => writeText(SHELL_ASK_KEY, v) },
    now: Date.now,
    loadedAt,
    hasBeenActive: () => navigator.userActivation?.hasBeenActive ?? true,
    onView: (v) => (view = v),
  })

  $effect(() => {
    if (!inDesktopShell()) return
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const onInput = () => {
      window.removeEventListener('pointerdown', onInput, true)
      window.removeEventListener('keydown', onInput, true)
      void run()
    }
    const waitForInput = () => {
      window.addEventListener('pointerdown', onInput, true)
      window.addEventListener('keydown', onInput, true)
    }
    // Answered elsewhere (the launcher, another instance, System Settings):
    // the card comes down the next time this page is looked at. Once the
    // question is settled — allowed, refused, put off, or a shell with
    // nothing to ask — this page stops asking the shell.
    const onVisible = () => {
      if (!document.hidden) void ask.check().then((r) => r === 'no' && settle())
    }
    const settle = () => {
      document.removeEventListener('visibilitychange', onVisible)
      window.removeEventListener('focus', onVisible)
    }
    const run = async () => {
      const r = await ask.check()
      if (stopped) return
      if (r === 'no') return settle()
      if (r !== 'later') return
      const early = loadedAt + MIN_LOADED_MS - Date.now()
      if (early > 0) {
        clearTimeout(timer)
        timer = setTimeout(() => void run(), early + 50)
      } else waitForInput()
    }
    void run()
    document.addEventListener('visibilitychange', onVisible)
    window.addEventListener('focus', onVisible)
    return () => {
      stopped = true
      clearTimeout(timer)
      window.removeEventListener('pointerdown', onInput, true)
      window.removeEventListener('keydown', onInput, true)
      settle()
    }
  })

  // Into the toast stack's lead slot (Toasts.svelte); on its own at the
  // corner if that host is not mounted.
  function intoStack(node: HTMLElement) {
    const slot = document.querySelector<HTMLElement>('[data-toast-lead]')
    if (slot) slot.prepend(node)
    else {
      node.classList.add('fixed', 'bottom-4', 'right-4', 'z-[70]', 'w-[min(92vw,340px)]')
      document.body.appendChild(node)
    }
    return { destroy: () => node.remove() }
  }
</script>

{#if view.kind !== 'hidden'}
  <div
    use:intoStack
    in:fly={{ y: 8, duration: 220 }}
    out:fade={QUICK}
    class="pointer-events-auto"
    role="dialog"
    aria-label="Allow notifications"
    data-desktop-notify-ask={view.kind}
  >
    <Panel class="p-4 shadow-[var(--theme-shadow-2)]">
      {#if view.kind === 'blocked'}
        <div class="font-sans text-sm font-medium text-fg">Notifications are off for Talaria</div>
        <div class="mt-1.5 font-sans text-xs text-muted">
          Turn them on in System Settings → Notifications → Talaria. Banners start as soon as you do.
        </div>
        <div class="mt-3 flex justify-end">
          <Button variant="ghost" size="sm" onclick={() => ask.close()}>Close</Button>
        </div>
      {:else}
        <div class="font-sans text-sm font-medium text-fg">Allow notifications</div>
        <div class="mt-1.5 font-sans text-xs text-muted">
          Talaria posts a banner when a message or an approval needs you, from every instance you open. {view.os} asks
          you first.
        </div>
        {#if view.message}
          <div class="mt-1.5 font-sans text-xs text-muted" data-desktop-notify-ask-message>{view.message}</div>
        {/if}
        <div class="mt-3 flex justify-end gap-2">
          <Button variant="ghost" size="sm" disabled={view.busy} onclick={() => ask.notNow()}>Not now</Button>
          <Button size="sm" disabled={view.busy} onclick={() => void ask.allow()}>
            {view.busy ? `Waiting for ${view.os}` : 'Allow notifications'}
          </Button>
        </div>
      {/if}
    </Panel>
  </div>
{/if}
