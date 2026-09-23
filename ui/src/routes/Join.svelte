<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import { navigate } from '@/router'
  import Brand from '@/components/Brand.svelte'
  import MercuryBackdrop from '@/components/MercuryBackdrop.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { getJson, HttpError } from '@/lib/fetch-json'
  import { useSession } from '@/lib/session'

  interface Invite {
    email: string
    invitedBy: string | null
    orgName: string
  }

  // The invite landing page: /join?token=… shows who invited you and to what,
  // then hands off to Google sign-in — the invite itself is what admits the
  // email at the auth gate.
  const token = $derived(searchParams.get('token') ?? undefined)

  const session = useSession()
  $effect(() => {
    if (session.isSuccess && session.data) navigate('/')
  })

  const query = createQuery(() => ({
    queryKey: ['join', token],
    enabled: !!token,
    queryFn: async (): Promise<Invite | null> => {
      // 404/410 are the invite's OWN answer — no such token, or it's spent —
      // and only those two earn the "no longer valid" sentence below. Every
      // other non-2xx is the box failing, and `if (!r.ok) return null` used to
      // fold all three together: a 500 during a deploy told a brand-new hire
      // their invite was dead, and sent them to an admin for a replacement
      // token that was never needed. Different failures, different sentences.
      try {
        return (await getJson<{ invite: Invite }>(`/api/join?token=${encodeURIComponent(token!)}`)).invite
      } catch (e) {
        if (e instanceof HttpError && (e.status === 404 || e.status === 410)) return null
        throw e
      }
    },
  }))
</script>

<MercuryBackdrop />
<div class="grid min-h-screen place-items-center p-6">
  <Panel class="w-full max-w-md p-8 text-center">
    <div class="mb-6 flex justify-center">
      <Brand size={40} />
    </div>
    <QueryState query={query} errorTitle="Could not check your invite">
      {#snippet skeleton()}
        <div class="space-y-3">
          <Skeleton class="mx-auto h-4 w-48 rounded-full" />
          <Skeleton class="mx-auto h-3 w-64 rounded-full" />
        </div>
      {/snippet}
      {#snippet idle()}
        <!-- No token at all is the surface's own "nothing selected" state, not
             a broken read — QueryState's default idle line would say less. -->
        <p class="text-sm text-muted">This join link is missing its token.</p>
      {/snippet}
      {#snippet empty()}
        <!-- `null` is the invite's OWN answer (404/410) — the link is spent or
             the token was never valid. -->
        <h1 class="mb-2 font-sans text-lg font-semibold text-fg">This invite is no longer valid</h1>
        <p class="font-sans text-sm text-muted">It may have expired or been revoked. Ask your admin for a fresh one.</p>
      {/snippet}
      {#snippet children(invite)}
        <!-- `empty` owns the spent/never-valid invite, so children only ever
             sees a live one. This is the narrowing the compiler needs. -->
        {#if invite}
          <h1 class="mb-2 font-sans text-lg font-semibold text-fg">Join {invite.orgName}</h1>
          <p class="mb-6 font-sans text-sm text-muted">
            {invite.invitedBy ? `${invite.invitedBy} invited you` : 'You were invited'} to {invite.orgName}'s Talaria workspace.
            Sign in with Google using <span class="font-mono text-[13px] text-fg">{invite.email}</span> and you're in.
          </p>
          <a href="/api/auth/google" class={buttonClasses({ className: 'w-full' })}>
            Continue with Google
          </a>
          <p class="mt-4 font-sans text-[11px] text-muted/80">
            The invite is bound to the email address, so signing in with a different account won't use it.
          </p>
        {/if}
      {/snippet}
    </QueryState>
  </Panel>
</div>
