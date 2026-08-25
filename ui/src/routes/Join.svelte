<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import { navigate } from '@/router'
  import Brand from '@/components/Brand.svelte'
  import MercuryBackdrop from '@/components/MercuryBackdrop.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { errorMessage, readJson } from '@/lib/fetch-json'
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
      const r = await fetch(`/api/join?token=${encodeURIComponent(token!)}`, { credentials: 'same-origin' })
      // 404/410 are the invite's OWN answer — no such token, or it's spent —
      // and only those two earn the "no longer valid" sentence below. Every
      // other non-2xx is the box failing, and `if (!r.ok) return null` used to
      // fold all three together: a 500 during a deploy told a brand-new hire
      // their invite was dead, and sent them to an admin for a replacement
      // token that was never needed. Different failures, different sentences.
      if (r.status === 404 || r.status === 410) return null
      return (await readJson<{ invite: Invite }>(r)).invite
    },
  }))
</script>

<MercuryBackdrop />
<div class="grid min-h-screen place-items-center p-6">
  <Panel class="w-full max-w-md p-8 text-center">
    <div class="mb-6 flex justify-center">
      <Brand size={40} />
    </div>
    {#if !token}
      <p class="text-sm text-muted">This join link is missing its token.</p>
    {:else if query.isPending}
      <div class="space-y-3">
        <Skeleton class="mx-auto h-4 w-48 rounded-full" />
        <Skeleton class="mx-auto h-3 w-64 rounded-full" />
      </div>
    {:else if query.isError && query.data === undefined}
      <!-- Nothing is known about the invite here — so claim nothing about
           it. The reader's next move is "try again", not "email an admin".
           `data === undefined` per the house rule: a failed RE-fetch behind
           an invite already on screen must not blank it out. -->
      <h1 class="mb-2 font-sans text-lg font-semibold text-fg">Could not check your invite</h1>
      <p class="mb-1 font-sans text-sm text-muted">{errorMessage(query.error)}</p>
      <p class="mb-5 font-sans text-sm text-muted">
        Your invite is most likely fine. This is a problem reaching the server, not a problem with your link.
      </p>
      <Button variant="outline" size="sm" onclick={() => void query.refetch()}>
        Try again
      </Button>
    {:else if !query.data}
      <h1 class="mb-2 font-sans text-lg font-semibold text-fg">This invite is no longer valid</h1>
      <p class="font-sans text-sm text-muted">It may have expired or been revoked. Ask your admin for a fresh one.</p>
    {:else}
      <h1 class="mb-2 font-sans text-lg font-semibold text-fg">Join {query.data.orgName}</h1>
      <p class="mb-6 font-sans text-sm text-muted">
        {query.data.invitedBy ? `${query.data.invitedBy} invited you` : 'You were invited'} to {query.data.orgName}'s Talaria workspace.
        Sign in with Google using <span class="font-mono text-[13px] text-fg">{query.data.email}</span> and you're in.
      </p>
      <a href="/api/auth/google" class={buttonClasses({ className: 'w-full' })}>
        Continue with Google
      </a>
      <p class="mt-4 font-sans text-[11px] text-muted/80">
        The invite is bound to the email address, so signing in with a different account won't use it.
      </p>
    {/if}
  </Panel>
</div>
