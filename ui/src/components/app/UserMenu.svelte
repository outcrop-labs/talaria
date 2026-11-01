<script lang="ts">
  import { BellOff, ChevronRight, Cog, LogOut, Settings, Shield, SunMoon } from '@lucide/svelte'
  import ThemeToggle from '@/components/ThemeToggle.svelte'
  import PauseMenu from '@/components/notify/PauseMenu.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { SessionUser } from '@/lib/session'
  import { cn } from '@/lib/cn'
  import { useNotifySettings } from '@/lib/notify-center'
  import { dndView, safeZone } from '@/lib/notify-dnd'
  import { p } from '@/router'

  // THE DOCK'S ACCOUNT CHIP: status dot + display name, with everything
  // personal nested in a flyover (profile, theme, Settings, Admin, Manage,
  // sign out). Settings/Admin live HERE, not among the tiles: they're about
  // the person and the instance, not the work.
  //
  // `active` lights the chip while the route is inside one of the two views
  // this menu owns (/settings, /admin). They are the only views with no tile
  // anywhere in the dock, and neither renders a title of its own, so without
  // this nothing on the screen says where you are — the job the deleted
  // strip's "System" breadcrumb used to do.
  let {
    user,
    onLogout,
    active = false,
    hasManage = false,
  }: {
    user: SessionUser
    onLogout: () => void
    active?: boolean
    /** Whether this viewer has any manage view — the dock decides, once. */
    hasManage?: boolean
  } = $props()

  // PAUSE NOTIFICATIONS lives here too (Slack's place for it): the menu that
  // is about the person. Read from the bell's cached payload — no poll of its
  // own — and expanded in place, since this menu is a popover already.
  const notify = useNotifySettings()
  const dnd = $derived(notify.data?.dnd)
  let pauseOpen = $state(false)
  const pauseLine = $derived(dndView(dnd, Date.now(), safeZone(dnd?.timeZone)))
</script>

<!-- The §7 popover shell owns the mechanics (outside-click, Esc, portal);
     offset 8 keeps the gap the old mt-2 panel sat at. -->
<Popover align="right" offset={8} class="w-64">
  {#snippet trigger(open)}
    <button
      type="button"
      aria-expanded={open}
      class={cn(
        'flex h-9 items-center gap-2 rounded-md px-2 transition-colors duration-[120ms] dither-fill',
        active && 'bg-raised',
      )}
    >
      <StatusDot status="ok" />
      <!-- Display name, not email: the flyover header still shows both. -->
      <span
        class={cn(
          'hidden max-w-[16rem] truncate font-mono text-[11px] sm:block',
          active ? 'text-fg' : 'text-muted',
        )}
      >
        {user.name ?? user.email ?? 'Account'}
      </span>
    </button>
  {/snippet}
  {#snippet content(close)}
    <div class="flex items-center gap-2.5 border-b border-line px-3 py-2.5">
      <Avatar src={user.picture} name={user.name ?? user.email} class="h-8 w-8 shrink-0" />
      <div class="min-w-0">
        <div class="truncate text-sm font-medium text-fg">{user.name ?? user.email}</div>
        <div class="flex items-baseline gap-2">
          <span class="min-w-0 truncate font-mono text-[11px] text-muted">{user.email}</span>
          {#if user.role === 'admin'}
            <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-accent">admin</span>
          {/if}
        </div>
      </div>
    </div>

    <a
      href={p('/settings')}
      onclick={close}
      class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm text-fg transition-colors duration-[120ms] dither-fill"
    >
      <Settings size={15} class="shrink-0 text-muted" />
      <span>Settings</span>
    </a>
    {#if user.role === 'admin'}
      <a
        href={p('/admin')}
        onclick={close}
        class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm text-fg transition-colors duration-[120ms] dither-fill"
      >
        <Shield size={15} class="shrink-0 text-muted" />
        <span>Admin</span>
      </a>
    {/if}

    <!-- Manage goes to the control plane's overview, the same place the
         dock's gear goes; the gear glyph matches that tile. Shown to whoever
         the gear is shown to. -->
    {#if hasManage}
      <a
        href={p('/manage')}
        onclick={close}
        class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm text-fg transition-colors duration-[120ms] dither-fill"
      >
        <Cog size={15} class="shrink-0 text-muted" />
        <span>Manage</span>
      </a>
    {/if}
    {#if dnd}
      <button
        type="button"
        aria-expanded={pauseOpen}
        onclick={() => (pauseOpen = !pauseOpen)}
        class="flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm text-fg transition-colors duration-[120ms] dither-fill"
        data-user-menu-pause
      >
        <BellOff size={15} class="shrink-0 {pauseLine.active ? 'text-accent' : 'text-muted'}" />
        <span class="min-w-0 flex-1 truncate">{pauseLine.active && pauseLine.reason === 'paused' ? pauseLine.label : 'Pause notifications'}</span>
        <ChevronRight size={13} class={cn('shrink-0 text-muted transition-transform duration-150', pauseOpen && 'rotate-90')} />
      </button>
      {#if pauseOpen}
        <div class="mx-1 mb-1 rounded-md border border-line-subtle py-1">
          <PauseMenu
            {dnd}
            onDone={() => {
              pauseOpen = false
              close()
            }}
          />
        </div>
      {/if}
    {/if}
    <!-- Theme toggles in place — the flyover stays open for it. -->
    <div class="flex items-center justify-between rounded-md px-3 py-1.5 text-sm text-muted">
      <span class="flex items-center gap-2.5">
        <SunMoon size={15} class="shrink-0 text-muted" />
        <span>Theme</span>
      </span>
      <ThemeToggle />
    </div>

    <div class="mt-1 border-t border-line pt-1">
      <!-- No close(): sign-out tears the session down and leaves the page —
           the flyover never outlives it. -->
      <button
        type="button"
        onclick={onLogout}
        class="flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm text-[color:var(--theme-danger)] transition-colors duration-[120ms] hover:bg-[color:var(--theme-danger)]/10"
      >
        <LogOut size={15} class="shrink-0" />
        <span>Sign out</span>
      </button>
    </div>
  {/snippet}
</Popover>
