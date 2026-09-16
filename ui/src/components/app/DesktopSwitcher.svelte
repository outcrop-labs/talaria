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

  // The desktop instance switcher — exists ONLY inside the Talaria desktop
  // shell, where the shell grants this origin three commands. In a browser
  // `inDesktopShell` is false and this component renders nothing at all.
  // It wears the CURRENT instance's identity (avatar + label), because an
  // unexplained icon is not a switcher.
  let { collapsed = false }: { collapsed?: boolean } = $props()

  let instances = $state<ShellInstance[]>([])
  const branding = useInstanceBranding()

  $effect(() => {
    if (!inDesktopShell()) return
    void shellListInstances().then((list) => (instances = list))
  })

  const current = $derived(
    instances.find((i) => i.instanceId === branding.data?.instance),
  )
  const initial = $derived((current?.label ?? 'T').trim().charAt(0).toUpperCase())

  // A function, so the menu re-reads the live list on every open — switching
  // elsewhere (or adding) shouldn't leave this dropdown stale.
  const items = (): ContextMenuEntry[] => [
    ...instances.map((instance): ContextMenuEntry => ({
      label: instance.label,
      checked: instance.instanceId === branding.data?.instance,
      onSelect: () => void shellActivateInstance(instance.id),
    })),
    ...(instances.length ? ['sep' as const] : []),
    { label: 'Add or manage instances…', onSelect: () => void shellShowWelcome() },
  ]
</script>

{#if inDesktopShell()}
  <DropdownMenu {items} align={collapsed ? 'left' : 'right'}>
    {#snippet trigger(open: boolean)}
      <button
        type="button"
        title={current ? `Instances — showing ${current.label}` : 'Instances'}
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
          <span class="whitespace-nowrap text-[11px] leading-none">{current?.label ?? 'Instance'}</span>
          <ChevronDown size={11} strokeWidth={1.5} class="shrink-0 opacity-70" />
        {/if}
      </button>
    {/snippet}
  </DropdownMenu>
{/if}
