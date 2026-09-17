<script lang="ts">
  import { ChevronDown } from '@lucide/svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useInstanceBranding } from '@/lib/instance-branding.svelte'
  import { cn } from '@/lib/cn'
  import {
    inDesktopShell,
    shellActivateInstance,
    shellListInstances,
    shellShowWelcome,
    type ShellInstance,
  } from '@/lib/desktop-shell'
  import { findCurrentInstance, instanceDisplayLabel } from '@/lib/desktop-instance'

  // The desktop instance switcher — exists ONLY inside the Talaria desktop
  // shell, where the shell grants this origin three commands. In a browser
  // `inDesktopShell` is false and this component renders nothing at all.
  // It wears the CURRENT instance's identity (avatar + label), because an
  // unexplained icon is not a switcher.
  let { collapsed = false }: { collapsed?: boolean } = $props()

  let instances = $state<ShellInstance[]>([])
  const branding = useInstanceBranding()

  const refresh = async (): Promise<void> => {
    if (!inDesktopShell()) return
    instances = await shellListInstances()
  }

  $effect(() => {
    void refresh()
  })

  const current = $derived(
    findCurrentInstance(
      instances,
      branding.data?.instance,
      typeof window === 'undefined' ? '' : window.location.origin,
    ),
  )
  const currentLabel = $derived(current ? instanceDisplayLabel(current) : 'Instance')
  const initial = $derived(currentLabel.trim().charAt(0).toUpperCase())

  // A function, so the menu re-reads the live list on every open — switching
  // elsewhere (or adding) shouldn't leave this dropdown stale. Labels fall
  // back to the host so an instance with no company name still has a row.
  const items = (): ContextMenuEntry[] => [
    ...instances.map((instance): ContextMenuEntry => ({
      label: instanceDisplayLabel(instance),
      checked: instance.id === current?.id,
      onSelect: () => void shellActivateInstance(instance.id),
    })),
    ...(instances.length ? ['sep' as const] : []),
    { label: 'Add or manage instances', onSelect: () => void shellShowWelcome() },
  ]
</script>

{#if inDesktopShell()}
  <DropdownMenu {items} align="left" onWillOpen={refresh}>
    {#snippet trigger(open: boolean)}
      <button
        type="button"
        title={current ? `Instances — showing ${currentLabel}` : 'Instances'}
        aria-label="Switch instance"
        class={cn(
          'flex items-center gap-1.5 rounded-md text-muted transition-colors duration-[120ms] hover:text-fg',
          open && 'text-fg',
          collapsed
            ? 'relative h-9 w-9 justify-center bg-raised data-[open]:bg-raised'
            : 'h-6 px-1.5',
        )}
      >
        <span
          class={cn(
            'grid shrink-0 place-items-center rounded-[5px] bg-accent font-display text-[10px] font-semibold leading-none text-bg',
            collapsed ? 'h-6 w-6 text-[11px]' : 'h-[18px] w-[18px]',
          )}
        >
          {initial}
        </span>
        {#if !collapsed}
          <span class="whitespace-nowrap text-[11px] leading-none">{currentLabel}</span>
          <ChevronDown size={11} strokeWidth={1.5} class="shrink-0 opacity-70" />
        {/if}
      </button>
    {/snippet}
  </DropdownMenu>
{/if}
