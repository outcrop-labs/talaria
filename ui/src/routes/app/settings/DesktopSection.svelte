<script lang="ts">
  import Panel from '@/components/ui/Panel.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import {
    desktopSettings,
    loadDesktopSettings,
    setCloseToBackground,
    setSuspendAfter,
    setTitlebarMode,
  } from '@/lib/desktop-settings.svelte'
  import {
    shellCheckForUpdate,
    shellInstallUpdate,
    shellNotificationStatus,
    shellRequestNotificationPermission,
    shellSendTestNotification,
    type DesktopUpdate,
    type ShellNotificationStatus,
    type TitlebarMode,
  } from '@/lib/desktop-shell'

  // Desktop-shell window chrome and in-app updates. Only mounted when
  // `inDesktopShell()` — a browser has no titlebar to pick and no installer
  // to replace.
  const settings = $derived(desktopSettings())
  let update = $state<DesktopUpdate | null>(null)
  let updateBusy = $state(false)
  let updateMsg = $state('')

  $effect(() => {
    void loadDesktopSettings()
  })

  const options: { id: TitlebarMode; label: string; title: string }[] = [
    { id: 'themed', label: 'Themed', title: 'Themed window controls, drag the bar to move' },
    { id: 'os', label: 'OS', title: 'The operating system titlebar' },
    { id: 'none', label: 'None', title: 'No titlebar' },
  ]

  // How long an instance may stay off screen before the shell gives back its
  // memory (desktop/src-tauri/src/suspend.rs). Shown only by a shell that
  // has the setting; a value set by hand that is not listed shows as its own.
  const suspendChoices: { id: string; label: string; title: string }[] = [
    { id: '0', label: 'Off', title: 'Keep every opened instance loaded' },
    { id: '5', label: '5 min', title: 'Suspend after 5 minutes off screen' },
    { id: '10', label: '10 min', title: 'Suspend after 10 minutes off screen' },
    { id: '30', label: '30 min', title: 'Suspend after 30 minutes off screen' },
    { id: '60', label: '1 h', title: 'Suspend after an hour off screen' },
  ]
  const suspendValue = $derived(
    settings.suspendAfterMinutes === undefined ? null : String(settings.suspendAfterMinutes),
  )
  const suspendOptions = $derived(
    suspendValue === null || suspendChoices.some((c) => c.id === suspendValue)
      ? suspendChoices
      : [...suspendChoices, { id: suspendValue, label: `${suspendValue} min`, title: 'Set in settings.json' }].sort(
          (a, b) => Number(a.id) - Number(b.id),
        ),
  )
  let suspendMsg = $state('')
  const chooseSuspend = async (id: string) => {
    suspendMsg = ''
    try {
      await setSuspendAfter(Number(id))
    } catch (e) {
      suspendMsg = e instanceof Error ? e.message : String(e)
    }
  }

  // OS notifications, as the shell reads them from the OS (D9): what posts
  // them, whether the OS lets it, a real test banner, and what closing the
  // window does. Null: a shell that predates the commands, and the section
  // stays hidden. Nothing here is assumed: no service, no "On".
  let notify = $state<ShellNotificationStatus | null>(null)
  let notifyBusy = $state(false)
  let notifyMsg = $state('')
  let notifyFailed = $state(false)

  const readNotify = async () => {
    try {
      notify = await shellNotificationStatus()
    } catch (e) {
      notifyMsg = e instanceof Error ? e.message : String(e)
      notifyFailed = true
    }
  }

  $effect(() => {
    void readNotify()
    const again = () => {
      if (!document.hidden) void readNotify()
    }
    document.addEventListener('visibilitychange', again)
    return () => document.removeEventListener('visibilitychange', again)
  })

  const shownBy = (s: ShellNotificationStatus): string => {
    if (s.server) return `${s.server.name} ${s.server.version} (spec ${s.server.spec})`
    if (s.backend === 'un') return 'macOS Notification Center'
    if (s.backend.startsWith('winrt')) return 'Windows notifications'
    return s.backend
  }

  const notifyLine = $derived.by((): { text: string; tone: 'ok' | 'warn' | 'bad' } | null => {
    if (!notify) return null
    switch (notify.permission) {
      case 'granted':
        return { text: `On — shown by ${shownBy(notify)}`, tone: 'ok' }
      case 'not-determined':
        return { text: 'Not allowed yet', tone: 'warn' }
      case 'denied':
        return { text: 'Blocked in System Settings', tone: 'bad' }
      default:
        return {
          text: `Notification service unavailable: ${(notify.detail ?? 'no reason given').replace(/^unavailable:\s*/, '')}`,
          tone: 'bad',
        }
    }
  })

  const backgroundLine = $derived.by((): string => {
    switch (notify?.background) {
      case 'tray':
        return 'Closing the window keeps Talaria in the tray. Quit from the tray menu, the dock, Ctrl+Q, or the switcher.'
      case 'menu-bar':
        return 'Closing the window keeps Talaria in the dock and the menu bar. Quit with Cmd+Q or the menu bar.'
      case 'minimize':
        return 'Closing the window minimizes Talaria — this desktop shows no tray icons. Quit from the dock, with Ctrl+Q, or the switcher.'
      default:
        return 'On this platform, closing the window still quits Talaria.'
    }
  })

  const allow = async () => {
    notifyBusy = true
    notifyMsg = ''
    notifyFailed = false
    try {
      notify = (await shellRequestNotificationPermission()) ?? notify
    } catch (e) {
      notifyMsg = e instanceof Error ? e.message : String(e)
      notifyFailed = true
    } finally {
      notifyBusy = false
    }
  }

  const test = async () => {
    notifyBusy = true
    notifyMsg = ''
    notifyFailed = false
    try {
      const result = await shellSendTestNotification()
      if (!result) return
      notify = result.status
      if (result.ok) notifyMsg = `Sent — shown by ${shownBy(result.status)}`
      else {
        notifyMsg = `Not shown: ${result.error ?? 'the notification service refused it'}`
        notifyFailed = true
      }
    } catch (e) {
      notifyMsg = e instanceof Error ? e.message : String(e)
      notifyFailed = true
    } finally {
      notifyBusy = false
    }
  }

  const chooseBackground = async (on: boolean) => {
    try {
      await setCloseToBackground(on)
      await readNotify()
    } catch (e) {
      notifyMsg = e instanceof Error ? e.message : String(e)
      notifyFailed = true
    }
  }

  const check = async () => {
    updateBusy = true
    updateMsg = ''
    update = null
    try {
      const found = await shellCheckForUpdate()
      if (found) update = found
      else updateMsg = 'You are on the latest version'
    } catch (e) {
      updateMsg = e instanceof Error ? e.message : String(e)
    } finally {
      updateBusy = false
    }
  }

  const install = async () => {
    updateBusy = true
    updateMsg = ''
    try {
      await shellInstallUpdate()
    } catch (e) {
      updateMsg = e instanceof Error ? e.message : String(e)
      updateBusy = false
    }
  }
</script>

<Panel as="section">
  <div class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Window</div>
  <p class="mb-3 max-w-xl font-sans text-sm text-muted">
    Titlebar for the desktop app. Themed is the default: window controls that
    match this UI, and a bar you can drag. OS uses the operating system
    titlebar. None hides both.
  </p>
  <Segmented
    {options}
    value={settings.titlebar}
    onChange={(id) => void setTitlebarMode(id)}
  />
</Panel>

{#if suspendValue !== null}
  <Panel as="section">
    <div class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Inactive instances</div>
    <p class="mb-3 max-w-xl font-sans text-sm text-muted">
      An instance you have not had on screen for this long gives back its
      memory, and opens again at the same page the moment you return to it.
      A page with unsent typing stays loaded. The window you are looking at
      counts only while it is minimized.
    </p>
    <Segmented options={suspendOptions} value={suspendValue} onChange={(id) => void chooseSuspend(id)} />
    {#if suspendMsg}
      <p class="mt-2 font-sans text-sm text-danger">{suspendMsg}</p>
    {/if}
  </Panel>
{/if}

{#if notify}
  <Panel as="section">
    <div class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Notifications</div>
    <p class="mb-3 max-w-xl font-sans text-sm text-muted">
      The desktop app posts banners through your operating system, for every
      instance you are signed in to, open or not. A banner closes by itself
      when you read its notification anywhere else. The dock badge counts
      what is for you — what needs you and what is addressed to you. What the
      OS says right now:
    </p>
    {#if notifyLine}
      <p
        class="mb-1 font-sans text-sm {notifyLine.tone === 'ok'
          ? 'text-fg'
          : notifyLine.tone === 'warn'
            ? 'text-muted'
            : 'text-danger'}"
        data-testid="desktop-notify-status"
      >
        {notifyLine.text}
      </p>
    {/if}
    {#if notify.muted}
      <p class="mb-1 max-w-xl font-sans text-sm text-fg" data-testid="desktop-notify-muted">
        This instance is muted in the launcher: it posts no banners on this desktop. Its unread
        count still counts toward the dock badge. Unmute it from the launcher's instance list.
      </p>
    {/if}
    {#if notify.lastError}
      <p class="mb-1 font-sans text-xs text-danger">Last error: {notify.lastError}</p>
    {:else if notify.pending}
      <p class="mb-1 font-sans text-xs text-muted">
        The last banner was handed over and had not been confirmed when Talaria checked.
      </p>
    {/if}
    <div class="mt-3 flex flex-wrap items-center gap-3">
      {#if notify.permission === 'not-determined'}
        <Button size="sm" disabled={notifyBusy} onclick={() => void allow()}>Allow notifications</Button>
      {:else if notify.permission === 'denied'}
        <Button size="sm" variant="outline" disabled={notifyBusy} onclick={() => void allow()}>
          Open System Settings
        </Button>
      {/if}
      <Button size="sm" variant="outline" disabled={notifyBusy} onclick={() => void test()}>
        Send test notification
      </Button>
      {#if notifyMsg}
        <span class="font-sans text-sm {notifyFailed ? 'text-danger' : 'text-muted'}" data-testid="desktop-notify-result">
          {notifyMsg}
        </span>
      {/if}
    </div>
    {#if notify.background !== 'none'}
      <div class="mt-4">
        <Toggle
          checked={notify.closeToBackground}
          onChange={(on) => void chooseBackground(on)}
          label="Keep running in the background when the window closes"
        />
        <p class="mt-1 max-w-xl font-sans text-xs text-muted">
          {notify.closeToBackground ? backgroundLine : 'Closing the window quits Talaria, and no banners arrive until you open it again.'}
        </p>
      </div>
    {:else}
      <p class="mt-4 max-w-xl font-sans text-xs text-muted">{backgroundLine}</p>
    {/if}
  </Panel>
{/if}

<Panel as="section">
  <div class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Updates</div>
  <p class="mb-3 max-w-xl font-sans text-sm text-muted">
    Installs the latest stable desktop build from GitHub, signed, without a
    trip to the releases page. The app relaunches when it is done.
  </p>
  <div class="flex flex-wrap items-center gap-3">
    {#if update?.install === 'elsewhere'}
      <!-- A Linux package or an .msi install: the shell says what updates it
           (and refuses install_update), so there is no Install to offer. -->
      <span class="max-w-xl font-sans text-sm text-fg" data-testid="desktop-update-elsewhere">
        Talaria Desktop {update.version} is available. {update.how}
      </span>
    {:else if update}
      <Button size="sm" disabled={updateBusy} onclick={() => void install()}>
        {updateBusy ? 'Installing' : `Install ${update.version}`}
      </Button>
    {:else}
      <Button size="sm" variant="outline" disabled={updateBusy} onclick={() => void check()}>
        {updateBusy ? 'Checking' : 'Check for updates'}
      </Button>
    {/if}
    {#if updateMsg}
      <span class="font-sans text-sm text-muted">{updateMsg}</span>
    {/if}
  </div>
</Panel>
