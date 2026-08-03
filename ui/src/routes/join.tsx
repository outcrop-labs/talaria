import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { useQuery } from '@tanstack/react-query'
import { z } from 'zod'
import { MercuryBackdrop } from '@/components/mercury-backdrop'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
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
export const Route = createFileRoute('/join')({
  validateSearch: z.object({ token: z.string().optional() }),
  component: JoinPage,
})

function JoinPage() {
  const { token } = Route.useSearch()
  const { data: user, isSuccess } = useSession()
  const navigate = useNavigate()
  useEffect(() => {
    if (isSuccess && user) void navigate({ to: '/' })
  }, [isSuccess, user, navigate])

  const query = useQuery({
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
  })
  const { data, isPending } = query

  return (
    <>
      <MercuryBackdrop />
      <div className="grid min-h-screen place-items-center p-6">
        <div className="mercury-panel w-full max-w-md rounded-2xl p-8 text-center">
          {!token ? (
            <p className="text-sm text-muted">This join link is missing its token.</p>
          ) : isPending ? (
            <div className="space-y-3">
              <Skeleton className="mx-auto h-4 w-48 rounded-full" />
              <Skeleton className="mx-auto h-3 w-64 rounded-full" delay={0.12} />
            </div>
          ) : query.isError && data === undefined ? (
            // Nothing is known about the invite here — so claim nothing about
            // it. The reader's next move is "try again", not "email an admin".
            // `data === undefined` per the house rule: a failed RE-fetch behind
            // an invite already on screen must not blank it out.
            <>
              <h1 className="mb-2 font-sans text-lg font-semibold text-fg">Could not check your invite</h1>
              <p className="mb-1 font-sans text-sm text-muted">{errorMessage(query.error)}</p>
              <p className="mb-5 font-sans text-sm text-muted">
                Your invite is most likely fine — this is a problem reaching the server, not a problem with your link.
              </p>
              <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                Try again
              </Button>
            </>
          ) : !data ? (
            <>
              <h1 className="mb-2 font-sans text-lg font-semibold text-fg">This invite is no longer valid</h1>
              <p className="font-sans text-sm text-muted">It may have expired or been revoked — ask your admin for a fresh one.</p>
            </>
          ) : (
            <>
              <h1 className="mb-2 font-sans text-lg font-semibold text-fg">Join {data.orgName}</h1>
              <p className="mb-6 font-sans text-sm text-muted">
                {data.invitedBy ? `${data.invitedBy} invited you` : 'You were invited'} to {data.orgName}'s Talaria workspace.
                Sign in with Google using <span className="text-fg">{data.email}</span> and you're in.
              </p>
              <a
                href="/api/auth/google"
                className="inline-block rounded-xl bg-accent px-5 py-2.5 text-sm font-medium text-surface transition-opacity hover:opacity-90"
              >
                Continue with Google
              </a>
              <p className="mt-4 font-sans text-[11px] text-muted/80">
                The invite is bound to the email address — signing in with a different account won't use it.
              </p>
            </>
          )}
        </div>
      </div>
    </>
  )
}
