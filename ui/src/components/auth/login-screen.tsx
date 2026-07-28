import { useState } from 'react'
import { motion } from 'framer-motion'
import { useQueryClient } from '@tanstack/react-query'
import { Brand } from '@/components/brand'
import { ThemeToggle } from '@/components/theme-toggle'
import { Button, buttonClasses } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Panel } from '@/components/ui/panel'
import { Skeleton } from '@/components/ui/skeleton'
import { useProviders } from '@/lib/session'

const ERROR_COPY: Record<string, string> = {
  google_denied: 'Google sign-in was cancelled.',
  google_disabled: 'Google sign-in is not enabled.',
  bad_state: 'Sign-in expired or was tampered with. Please try again.',
  exchange_failed: 'Could not complete Google sign-in. Please try again.',
  not_allowed: 'That account is not allowed to access this Talaria.',
}

export function LoginScreen({ error }: { error?: string }) {
  const { data, isLoading } = useProviders()
  const providers = data?.providers ?? []
  const configured = data?.configured ?? true

  const hasGoogle = providers.some((p) => p.id === 'google')
  const hasPassword = providers.some((p) => p.id === 'password')

  return (
    <div className="relative flex min-h-screen items-center justify-center px-4">
      <div className="absolute right-4 top-4">
        <ThemeToggle />
      </div>

      <motion.div
        initial={{ opacity: 0, y: 16 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.5, ease: 'easeOut' }}
        className="w-full max-w-sm"
      >
        <Panel className="p-8">
          <div className="mb-6 flex flex-col items-center gap-2 text-center">
            <Brand showTag className="flex-col" />
          </div>

          <p className="mb-6 text-center text-sm text-muted">Sign in to command your fleet.</p>

          {error && ERROR_COPY[error] && (
            <div
              className="mb-4 rounded-lg border px-3 py-2 text-center text-sm"
              style={{
                borderColor: 'var(--theme-danger)',
                color: 'var(--theme-danger)',
                background: 'color-mix(in srgb, var(--theme-danger) 8%, transparent)',
              }}
            >
              {ERROR_COPY[error]}
            </div>
          )}

          {!configured && (
            <div className="mb-4 rounded-lg border border-line px-3 py-2 text-center text-xs text-muted">
              Server auth isn’t configured yet (set <code>AUTH_SECRET</code> and enable a provider).
            </div>
          )}

          {isLoading ? (
            // The shape of the widest sign-in form (OAuth button, divider,
            // username/password, submit) so the swap doesn't jump.
            <div aria-hidden className="flex flex-col gap-3">
              <Skeleton className="h-11 w-full" />
              <div className="flex items-center gap-3 py-1">
                <Skeleton className="h-px flex-1" />
                <Skeleton className="h-2.5 w-6 rounded-full" delay={0.12} />
                <Skeleton className="h-px flex-1" />
              </div>
              <Skeleton className="h-11 w-full" delay={0.12} />
              <Skeleton className="h-11 w-full" delay={0.24} />
              <Skeleton className="h-11 w-full" delay={0.36} />
            </div>
          ) : providers.length === 0 ? (
            <div className="py-4 text-center text-sm text-muted">No sign-in providers are enabled.</div>
          ) : (
            <div className="flex flex-col gap-3">
              {hasGoogle && <GoogleButton />}
              {hasGoogle && hasPassword && <Divider />}
              {hasPassword && <PasswordForm />}
            </div>
          )}
        </Panel>
      </motion.div>
    </div>
  )
}

function GoogleButton() {
  // OAuth start is a real navigation → an anchor, styled via the shared helper
  // so it matches Button exactly without duplicating classes.
  return (
    <a href="/api/auth/google" className={buttonClasses({ variant: 'outline', className: 'w-full' })}>
      <GoogleIcon className="h-5 w-5" />
      Continue with Google
    </a>
  )
}

function Divider() {
  return (
    <div className="flex items-center gap-3">
      <div className="h-px flex-1 border-t border-line" />
      <span className="text-xs uppercase tracking-wider text-muted">or</span>
      <div className="h-px flex-1 border-t border-line" />
    </div>
  )
}

function PasswordForm() {
  const qc = useQueryClient()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setErr(null)
    try {
      const res = await fetch('/api/auth/password', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify({ username, password }),
      })
      const data = (await res.json().catch(() => ({}))) as { ok?: boolean; error?: string }
      if (!res.ok || !data.ok) {
        setErr(data.error ?? 'Sign-in failed')
        return
      }
      await qc.invalidateQueries({ queryKey: ['session'] })
    } catch {
      setErr('Network error')
    } finally {
      setBusy(false)
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-3">
      <Input
        placeholder="Username"
        autoFocus
        autoComplete="username"
        value={username}
        onChange={(e) => setUsername(e.target.value)}
      />
      <Input
        placeholder="Password"
        type="password"
        autoComplete="current-password"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      {err && <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>{err}</div>}
      <Button type="submit" disabled={busy || !username || !password}>
        {busy ? 'Signing in' : 'Sign in'}
      </Button>
    </form>
  )
}

function GoogleIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden>
      <path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 0 1-2.2 3.32v2.77h3.57c2.08-1.92 3.27-4.74 3.27-8.1Z" />
      <path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.99.66-2.26 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84A11 11 0 0 0 12 23Z" />
      <path fill="#FBBC05" d="M5.84 14.1a6.6 6.6 0 0 1 0-4.2V7.06H2.18a11 11 0 0 0 0 9.88l3.66-2.84Z" />
      <path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1A11 11 0 0 0 2.18 7.06l3.66 2.84C6.71 7.31 9.14 5.38 12 5.38Z" />
    </svg>
  )
}
