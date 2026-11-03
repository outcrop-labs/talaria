<script lang="ts">
  import { ChevronDown } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import { useInstanceBranding } from '@/lib/instance-branding.svelte'
  import { cn } from '@/lib/cn'
  import {
    inDesktopShell,
    shellActivateInstance,
    shellListInstances,
    shellQuit,
    shellReload,
    shellSetBadge,
    shellShowWelcome,
    type ShellInstance,
  } from '@/lib/desktop-shell'
  import { badgeCount, createBadgeReporter } from '@/lib/desktop-badge'
  import { findCurrentInstance, instanceDisplayLabel, switcherMenu } from '@/lib/desktop-instance'
  import type { NotificationsPayload } from '@/lib/notifications'

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

  // The dock and launcher badge (interface I-5): this instance's For you
  // count (Needs you + Direct, EB-11; `unread` from an instance that predates
  // it). Read from the query cache the bell and the toasts already keep
  // fresh, rather than through a query of its own, so a signed-out page (the
  // login screen mounts this too) fetches nothing; lib/desktop-badge
  // debounces it and reports 0 when signed out. The shell keeps a share per
  // instance and shows the sum.
  const queries = inDesktopShell() ? useQueryClient() : null
  $effect(() => {
    if (!queries) return
    const badge = createBadgeReporter((count) => void shellSetBadge(count))
    const report = () =>
      badge.update(
        badgeCount(queries.getQueryData<NotificationsPayload>(['notifications'])),
        queries.getQueryData<{ user?: unknown }>(['session'])?.user === null,
      )
    report()
    const stop = queries.getQueryCache().subscribe((event) => {
      const key = event.query.queryKey[0]
      if (key === 'notifications' || key === 'session') report()
    })
    return () => {
      stop()
      badge.stop()
    }
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
  // What it holds and in what order: lib/desktop-instance `switcherMenu`.
  const items = () =>
    switcherMenu(instances, current?.id, navigator.userAgent, {
      activate: (id) => void shellActivateInstance(id),
      reload: () => void shellReload(),
      manage: () => void shellShowWelcome(),
      quit: () => void shellQuit(),
    })
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
