<script lang="ts">
  import { LogOut, Settings, Shield, SunMoon } from '@lucide/svelte'
  import ThemeToggle from '@/components/ThemeToggle.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { SessionUser } from '@/lib/session'
  import { p } from '@/router'

  // The strip's only control: status dot + display name, with everything
  // personal nested in a flyover (profile, theme, Settings, Admin, sign out).
  // Settings/Admin live HERE, not the rail: they're about the person and the
  // instance, not the work.
  let { user, onLogout }: { user: SessionUser; onLogout: () => void } = $props()
</script>

<!-- The §7 popover shell owns the mechanics (outside-click, Esc, portal);
     offset 8 keeps the gap the old mt-2 panel sat at. -->
<Popover align="right" offset={8} class="w-64">
  {#snippet trigger(open)}
    <button
      type="button"
      aria-expanded={open}
      class="flex h-7 items-center gap-2 rounded-md px-2 transition-colors duration-[120ms] dither-fill"
    >
      <StatusDot status="ok" />
      <!-- Display name, not email: the flyover header still shows both. -->
      <span class="hidden max-w-[16rem] truncate font-mono text-[11px] text-muted sm:block">
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
