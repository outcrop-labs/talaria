<script lang="ts">
  import Panel from '@/components/ui/Panel.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import {
    desktopSettings,
    loadDesktopSettings,
    setTitlebarMode,
  } from '@/lib/desktop-settings.svelte'
  import type { TitlebarMode } from '@/lib/desktop-shell'

  // Desktop-shell window chrome. Only mounted when `inDesktopShell()` — a
  // browser has no titlebar to pick. Themed is the default on every OS; OS
  // decorations and none are the opt-ins.
  const settings = $derived(desktopSettings())

  $effect(() => {
    void loadDesktopSettings()
  })

  const options: { id: TitlebarMode; label: string; title: string }[] = [
    { id: 'themed', label: 'Themed', title: 'Themed window controls, drag the bar to move' },
    { id: 'os', label: 'OS', title: 'The operating system titlebar' },
    { id: 'none', label: 'None', title: 'No titlebar' },
  ]
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
