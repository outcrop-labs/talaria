<script lang="ts">
  import { LogOut, Settings, Shield, SunMoon } from '@lucide/svelte'
  import ThemeToggle from '@/components/ThemeToggle.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import type { SessionUser } from '@/lib/session'
  import { p } from '@/router'

  // The strip's only control: status dot + display name, with everything
  // personal nested in a flyover (profile, theme, Settings, Admin, sign out).
  // Settings/Admin live HERE, not the rail: they're about the person and the
  // instance, not the work.
  let { user, onLogout }: { user: SessionUser; onLogout: () => void } = $props()

  let open = $state(false)
  let ref = $state<HTMLDivElement | null>(null)
</script>

<!-- Outside-click + Escape close via document listeners. -->
<svelte:document
  onmousedown={(e) => {
    if (open && !ref?.contains(e.target as Node)) open = false
  }}
  onkeydown={(e) => {
    if (open && e.key === 'Escape') open = false
  }}
/>

<div bind:this={ref} class="relative">
  <button data-dither-fill
    type="button"
    onclick={() => (open = !open)}
    class="flex h-7 items-center gap-2 rounded-md px-2 transition-colors duration-[120ms]"
  >
    <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-success" aria-hidden="true"></span>
    <!-- Display name, not email: the flyover header still shows both. -->
    <span class="hidden max-w-[16rem] truncate font-mono text-[11px] text-muted sm:block">
      {user.name ?? user.email ?? 'Account'}
    </span>
  </button>
  {#if open}
    <!-- Was the gd-enter CSS motif from the migration: entrance only, no exit,
         outside the @/lib/motion reduced-motion wrapper — swapped for the
         popover grammar. -->
    <div
      in:pop={POPOVER}
      out:fade={QUICK}
      class="absolute right-0 top-full z-30 mt-2 w-64 origin-top-right rounded-[10px] border border-line bg-panel p-1 shadow-[var(--theme-shadow-2)]"
    >
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

      <a data-dither-fill
        href={p('/settings')}
        onclick={() => (open = false)}
        class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm text-fg transition-colors duration-[120ms]"
      >
        <Settings size={15} class="shrink-0 text-muted" />
        <span>Settings</span>
      </a>
      {#if user.role === 'admin'}
        <a data-dither-fill
          href={p('/admin')}
          onclick={() => (open = false)}
          class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm text-fg transition-colors duration-[120ms]"
        >
          <Shield size={15} class="shrink-0 text-muted" />
          <span>Admin</span>
        </a>
      {/if}
      <div class="flex items-center justify-between rounded-md px-3 py-1.5 text-sm text-muted">
        <span class="flex items-center gap-2.5">
          <SunMoon size={15} class="shrink-0 text-muted" />
          <span>Theme</span>
        </span>
        <ThemeToggle />
      </div>

      <div class="mt-1 border-t border-line pt-1">
        <button
          type="button"
          onclick={onLogout}
          class="flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm text-[color:var(--theme-danger)] transition-colors duration-[120ms] hover:bg-[color:var(--theme-danger)]/10"
        >
          <LogOut size={15} class="shrink-0" />
          <span>Sign out</span>
        </button>
      </div>
    </div>
  {/if}
</div>
