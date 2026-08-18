<script lang="ts" module>
  const ERROR_COPY: Record<string, string> = {
    google_denied: 'Google sign-in was cancelled.',
    google_disabled: 'Google sign-in is not enabled.',
    bad_state: 'Sign-in expired or was tampered with. Please try again.',
    exchange_failed: 'Could not complete Google sign-in. Please try again.',
    not_allowed: 'That account is not allowed to access this Talaria.',
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

  let { error }: { error?: string } = $props()

  const providersQuery = useProviders()
  const providers = $derived(providersQuery.data?.providers ?? [])
  // Only a real 200 can claim auth is unconfigured. A failed provider read used
  // to render that warning, sending people to fix a config that was fine.
  const configured = $derived(providersQuery.data?.configured ?? true)

  const hasGoogle = $derived(providers.some((p) => p.id === 'google'))
  const hasPassword = $derived(providers.some((p) => p.id === 'password'))
</script>

<div class="relative flex min-h-screen items-center justify-center px-4">
  <div class="absolute right-4 top-4">
    <ThemeToggle />
  </div>

  <!-- |global: the card animates on COMPONENT mount (the route renders it
       unconditionally), so a local intro never plays. Hard page loads stay
       still anyway — mount() runs without intro. (ANIMATIONS.md) -->
  <div in:fly|global={PANEL} class="w-full max-w-sm">
    <Panel class="p-8">
      <div class="mb-6 flex flex-col items-center gap-2 text-center">
        <Brand showTag size={40} class="flex-col" />
      </div>

      <p class="mb-6 text-center text-sm text-muted">Sign in to command your fleet.</p>

      {#if error && ERROR_COPY[error]}
        <!-- Failure speaks safety-orange as an outline, never a fill (spec §8). -->
        <div transition:slide={{ duration: 150 }} class="mb-4 rounded-md border border-danger/40 px-3 py-2 text-center font-sans text-sm text-danger">
          {ERROR_COPY[error]}
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
          {#if hasGoogle && hasPassword}
            <!-- Divider between the OAuth and password routes. -->
            <div class="flex items-center gap-3">
              <div class="h-px flex-1 border-t border-line"></div>
              <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">or</span>
              <div class="h-px flex-1 border-t border-line"></div>
            </div>
          {/if}
          {#if hasPassword}<PasswordForm />{/if}
        </div>
      {/if}
    </Panel>
  </div>
</div>
