<script lang="ts">
  import { LogOut, SunMoon } from '@lucide/svelte'
  import ThemeToggle from '@/components/ThemeToggle.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import type { SessionUser } from '@/lib/session'

  // The strip's only control: status dot + account email, with everything
  // personal nested in a flyover (profile, theme, sign out). Settings and Admin
  // moved to the sidebar SYSTEM section — no duplicates here (spec §5).
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
  <button
    type="button"
    onclick={() => (open = !open)}
    class="flex h-7 items-center gap-2 rounded-md px-2 transition-colors duration-[120ms] hover:bg-hover"
  >
    <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-success" aria-hidden="true"></span>
    <span class="hidden max-w-[16rem] truncate font-mono text-[11px] text-muted sm:block">
      {user.email ?? user.name ?? 'Account'}
    </span>
  </button>
  {#if open}
    <div class="gd-enter absolute right-0 top-full z-30 mt-2 w-64 rounded-[10px] border border-line bg-panel p-1 shadow-[var(--theme-shadow-2)]">
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
