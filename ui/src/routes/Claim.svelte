<script lang="ts">
  import { navigate } from '@/router'
  import Brand from '@/components/Brand.svelte'
  import MercuryBackdrop from '@/components/MercuryBackdrop.svelte'
  import ClaimForm from '@/components/auth/ClaimForm.svelte'
  import GoogleButton from '@/components/auth/GoogleButton.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { useProviders, useSession } from '@/lib/session'

  // The first-run claim: a fresh Talaria has no admin, and the first person
  // through this page becomes it — email + password, or "Claim with Google"
  // when Google login is on. Reachable only while zero admins exist; the
  // server re-checks under a lock, so a lost race is a 409, never a second
  // admin.
  const session = useSession()
  const providersQuery = useProviders()
  const claimable = $derived(providersQuery.data?.claimable === true)
  const hasGoogle = $derived((providersQuery.data?.providers ?? []).some((p) => p.id === 'google'))

  $effect(() => {
    if (session.isSuccess && session.data) navigate('/')
  })
  // Claimed while this page was open (another tab, another operator, a Google
  // claim in another window): the gesture is over — sign in instead.
  $effect(() => {
    if (providersQuery.isSuccess && providersQuery.data && !claimable) navigate('/login')
  })
</script>

<MercuryBackdrop />
<div class="grid min-h-screen place-items-center p-6">
  <Panel class="w-full max-w-md p-8 text-center">
    <div class="mb-6 flex flex-col items-center gap-2">
      <Brand showTag size={40} class="flex-col" />
    </div>

    {#if providersQuery.isLoading}
      <div class="space-y-3">
        <Skeleton class="mx-auto h-4 w-48 rounded-full" />
        <Skeleton class="h-11 w-full" />
        <Skeleton class="h-11 w-full" />
        <Skeleton class="h-11 w-full" />
      </div>
    {:else if providersQuery.isError}
      <QueryError
        variant="compact"
        error={providersQuery.error}
        title="Could not check this instance"
        onRetry={() => void providersQuery.refetch()}
      />
    {:else if claimable}
      <h1 class="mb-2 font-sans text-lg font-semibold text-fg">Claim this instance</h1>
      <p class="mb-6 font-sans text-sm text-muted">
        This Talaria has no admin yet. The account you create here becomes the admin — whoever claims it, owns it.
      </p>
      <ClaimForm />
      {#if hasGoogle}
        <div class="my-4 flex items-center gap-3">
          <span class="h-px flex-1 bg-line"></span>
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">or</span>
          <span class="h-px flex-1 bg-line"></span>
        </div>
        <GoogleButton label="Claim with Google" />
        <p class="mt-4 font-sans text-[11px] text-muted/80">
          The first Google account through that button claims the instance.
        </p>
      {/if}
    {:else}
      <h1 class="mb-2 font-sans text-lg font-semibold text-fg">Already claimed</h1>
      <p class="font-sans text-sm text-muted">This instance has an admin. Redirecting to sign-in…</p>
    {/if}
  </Panel>
</div>
