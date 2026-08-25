<script lang="ts" module>
  const ERROR_COPY: Record<string, string> = {
    google_denied: 'Google sign-in was cancelled.',
    google_disabled: 'Google sign-in is not enabled.',
    bad_state: 'Sign-in expired or was tampered with. Please try again.',
    exchange_failed: 'Could not complete Google sign-in. Please try again.',
    not_allowed: 'That account is not allowed to access this Talaria.',
    // The domain ride-along (org.domain) personalizes this one when present.
    org_domain: 'Google sign-in is limited to this workspace’s accounts.',
  }
</script>

<script lang="ts">
  import Brand from '@/components/Brand.svelte'
  import ThemeToggle from '@/components/ThemeToggle.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import GoogleButton from './GoogleButton.svelte'
  import PasswordForm from './PasswordForm.svelte'
  import { fly, slide, PANEL } from '@/lib/motion'
  import { useProviders } from '@/lib/session'

  let { error, domain }: { error?: string; domain?: string } = $props()

  const providersQuery = useProviders()
  const providers = $derived(providersQuery.data?.providers ?? [])
  // Only a real 200 can claim auth is unconfigured. A failed provider read used
  // to render that warning, sending people to fix a config that was fine.
  const configured = $derived(providersQuery.data?.configured ?? true)

  const hasGoogle = $derived(providers.some((p) => p.id === 'google'))
  const hasPassword = $derived(providers.some((p) => p.id === 'password'))

  // When Google is connected it IS the sign-in: one button, nothing else on
  // the card. The password route stays reachable for the default org admin —
  // tucked into the bottom corner, disclosed on demand — because a workspace
  // that has gone all-in on Google login still needs its break-glass account,
  // and putting that form beside the button made every visitor read a second
  // first-class way in that exactly one person uses.
  let adminLogin = $state(false)
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && (adminLogin = false)} />

<div class="relative flex min-h-screen items-center justify-center px-4">
  <div class="absolute right-4 top-4">
    <ThemeToggle />
  </div>

  {#if hasGoogle && hasPassword}
    <!-- The password route's whole residence when Google owns the card: a
         mono whisper in the corner, disclosing the form on demand. Escape or
         the toggle itself puts it away. -->
    <div class="absolute bottom-4 left-4 flex flex-col items-start gap-2">
      {#if adminLogin}
        <div
          in:fly={{ y: 8, duration: 150 }}
          out:fly={{ y: 8, duration: 150 }}
          class="w-72 rounded-[10px] border border-line bg-panel p-4 shadow-[var(--theme-shadow-2)]"
        >
          <div class="mb-3 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Password sign-in</div>
          <PasswordForm />
        </div>
      {/if}
      <button
        type="button"
        aria-expanded={adminLogin}
        onclick={() => (adminLogin = !adminLogin)}
        class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors hover:text-fg"
      >
        {adminLogin ? 'Close' : 'Admin sign-in'}
      </button>
    </div>
  {/if}

  <!-- |global: the card animates on COMPONENT mount (the route renders it
       unconditionally), so a local intro never plays. Hard page loads stay
       still anyway — mount() runs without intro. (ANIMATIONS.md) -->
  <div in:fly|global={PANEL} class="w-full max-w-sm">
    <Panel class="p-8">
      <div class="mb-6 flex flex-col items-center gap-2 text-center">
        <Brand showTag size={40} class="flex-col" />
      </div>

      <p class="mb-6 text-center text-sm text-muted">Sign in and get to work.</p>

      {#if error && ERROR_COPY[error]}
        <!-- Failure speaks safety-orange as an outline, never a fill (spec §8). -->
        <div transition:slide={{ duration: 150 }} class="mb-4 rounded-md border border-danger/40 px-3 py-2 text-center font-sans text-sm text-danger">
          {error === 'org_domain' && domain ? `Sign in with your @${domain} Google account.` : ERROR_COPY[error]}
        </div>
      {/if}

      {#if !configured}
        <div class="mb-4 rounded-md border border-line px-3 py-2 text-center text-xs text-muted">
          Server auth isn’t configured yet (set <code>AUTH_SECRET</code> and enable a provider).
        </div>
      {/if}

      {#if providersQuery.isLoading}
        <!-- The shape of the widest sign-in form (OAuth button, divider,
             username/password, submit) so the swap doesn't jump. -->
        <div aria-hidden="true" class="flex flex-col gap-3">
          <Skeleton class="h-11 w-full" />
          <div class="flex items-center gap-3 py-1">
            <Skeleton class="h-px flex-1" />
            <Skeleton class="h-2.5 w-6 rounded-full" />
            <Skeleton class="h-px flex-1" />
          </div>
          <Skeleton class="h-11 w-full" />
          <Skeleton class="h-11 w-full" />
          <Skeleton class="h-11 w-full" />
        </div>
      {:else if providersQuery.isError}
        <QueryError
          variant="compact"
          error={providersQuery.error}
          title="Could not load sign-in options"
          onRetry={() => void providersQuery.refetch()}
        />
      {:else if providers.length === 0}
        <div class="py-4 text-center text-sm text-muted">No sign-in providers are enabled.</div>
      {:else}
        <div class="flex flex-col gap-3">
          {#if hasGoogle}<GoogleButton />{/if}
          {#if !hasGoogle && hasPassword}<PasswordForm />{/if}
        </div>
      {/if}
    </Panel>
  </div>
</div>
