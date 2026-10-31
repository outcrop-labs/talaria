<script lang="ts">
  import Panel from '@/components/ui/Panel.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import {
    desktopSettings,
    loadDesktopSettings,
    setSuspendAfter,
    setTitlebarMode,
  } from '@/lib/desktop-settings.svelte'
  import {
    shellCheckForUpdate,
    shellInstallUpdate,
    type DesktopUpdate,
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

<Panel as="section">
  <div class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Updates</div>
  <p class="mb-3 max-w-xl font-sans text-sm text-muted">
    Installs the latest stable desktop build from GitHub, signed, without a
    trip to the releases page. The app relaunches when it is done.
  </p>
  <div class="flex flex-wrap items-center gap-3">
    {#if update}
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
