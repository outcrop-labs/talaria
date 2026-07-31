import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { useQuery } from '@tanstack/react-query'
import { z } from 'zod'
import { Brand } from '@/components/brand'
import { MercuryBackdrop } from '@/components/mercury-backdrop'
import { buttonClasses } from '@/components/ui/button'
import { Panel } from '@/components/ui/panel'
import { Skeleton } from '@/components/ui/skeleton'
import { useSession } from '@/lib/session'

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

  const { data, isPending } = useQuery({
    queryKey: ['join', token],
    enabled: !!token,
    queryFn: async (): Promise<{ email: string; invitedBy: string | null; orgName: string } | null> => {
      const r = await fetch(`/api/join?token=${encodeURIComponent(token!)}`)
      if (!r.ok) return null
      return ((await r.json()) as { invite: { email: string; invitedBy: string | null; orgName: string } }).invite
    },
  })

  return (
    <>
      <MercuryBackdrop />
      <div className="grid min-h-screen place-items-center p-6">
        <Panel className="w-full max-w-md p-8 text-center">
          <div className="mb-6 flex justify-center">
            <Brand />
          </div>
          {!token ? (
            <p className="text-sm text-muted">This join link is missing its token.</p>
          ) : isPending ? (
            <div className="space-y-3">
              <Skeleton className="mx-auto h-4 w-48 rounded-full" />
              <Skeleton className="mx-auto h-3 w-64 rounded-full" delay={0.12} />
            </div>
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
                Sign in with Google using <span className="font-mono text-[13px] text-fg">{data.email}</span> and you're in.
              </p>
              <a href="/api/auth/google" className={buttonClasses({ className: 'w-full' })}>
                Continue with Google
              </a>
              <p className="mt-4 font-sans text-[11px] text-muted/80">
                The invite is bound to the email address — signing in with a different account won't use it.
              </p>
            </>
          )}
        </Panel>
      </div>
    </>
  )
}
