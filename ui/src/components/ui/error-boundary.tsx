import { Component, type ErrorInfo, type ReactNode } from 'react'
import { Button } from './button'
import { cn } from '@/lib/cn'

// The one error boundary. Talaria shipped without any: a render throw — most
// often a lazy chunk whose hash no longer exists after a deploy — white-screened
// the whole cockpit with no message and no way back. Every fallback here names
// WHAT broke and offers a way out (reload, retry). Never a stack trace: the
// stack goes to the console, the person gets a sentence and a button.

/** A chunk the server no longer has: the deploy moved out from under this tab. */
export function isStaleChunkError(error: unknown): boolean {
  const msg = error instanceof Error ? error.message : String(error ?? '')
  // "Unable to preload CSS for /assets/x-9f3c.css" is Vite's own preload helper
  // throwing for exactly this cause — the asset hash rotated under an open tab.
  // Same deploy, same fix; it must not fall through to the generic message.
  return /dynamically imported module|module script failed|Loading chunk|ChunkLoadError|Failed to fetch dynamically|unable to preload css/i.test(msg)
}

function messageOf(error: unknown): string {
  const msg = error instanceof Error ? error.message : typeof error === 'string' ? error : ''
  return msg.length > 220 ? `${msg.slice(0, 220)}…` : msg
}

/** The error zero-state. Shaped like EmptyState so a broken view reads like the
 *  rest of the app, not like a browser crash page. */
export function ErrorFallback({
  error,
  reset,
  what,
  variant = 'full',
  className,
}: {
  error: unknown
  /** Clears the error and re-renders. Omitted where retry can't help. */
  reset?: () => void
  /** What failed, in the user's words — "Boards", "this view". */
  what?: string
  variant?: 'full' | 'compact'
  className?: string
}) {
  const stale = isStaleChunkError(error)
  const subject = what ?? 'This view'
  const title = stale ? 'Talaria updated while this tab was open' : `${subject} failed to load`
  const hint = stale
    ? 'This tab is running an older build whose code is no longer on the server. Reload to pick up the new one.'
    : 'Something on this page failed while rendering. Reloading usually clears it; if it keeps happening, the detail below is what to report.'
  const detail = stale ? '' : messageOf(error)

  return (
    <div className={cn(variant === 'full' ? 'grid h-full min-h-[60vh] place-items-center p-6' : 'px-2 py-6', 'text-center', className)}>
      <div className={cn('max-w-sm', variant === 'compact' && 'mx-auto')}>
        {/* Not `mercury-text`: the brand gradient is for welcome moments. A
            failure gets the danger token, plainly. */}
        <div className={cn('mx-auto text-[color:var(--theme-danger)]', variant === 'full' ? 'mb-3 text-3xl' : 'mb-2 text-xl')}>⊘</div>
        <div className={cn('font-medium text-fg', variant === 'full' ? 'text-sm' : 'text-xs')}>{title}</div>
        <div className="mt-1 text-xs text-muted">{hint}</div>
        {detail && (
          <div className="mt-3 max-h-24 overflow-y-auto rounded-xl border border-line-subtle px-3 py-2 text-left text-[11px] break-words text-muted">
            {detail}
          </div>
        )}
        <div className={cn('flex flex-wrap items-center justify-center gap-2', variant === 'full' ? 'mt-4' : 'mt-3')}>
          <Button size="sm" onClick={() => window.location.reload()}>
            Reload
          </Button>
          {reset && !stale && (
            <Button size="sm" variant="outline" onClick={reset}>
              Try again
            </Button>
          )}
          {variant === 'full' && (
            <a href="/" className="text-xs text-accent hover:underline">
              Back to chat
            </a>
          )}
        </div>
      </div>
    </div>
  )
}

interface BoundaryProps {
  children: ReactNode
  /** What this boundary protects, named in the fallback ("Boards", "this view"). */
  what?: string
  variant?: 'full' | 'compact'
  /** Changing it clears a caught error — pass the identity of what's rendered
   *  inside, so navigating away from a broken surface heals the boundary. */
  resetKey?: string
  /** Render your own fallback instead of the default one. */
  fallback?: (error: unknown, reset: () => void) => ReactNode
}

interface BoundaryState {
  error: unknown
}

/** Catches render/lifecycle throws below it (React's only mechanism for this —
 *  hooks can't). Wrap every `Suspense` over a lazy chunk, and any subtree whose
 *  failure shouldn't take the app with it. */
export class ErrorBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { error: null }

  static getDerivedStateFromError(error: unknown): BoundaryState {
    return { error }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    // The one place a component stack is worth keeping — console, not the UI.
    console.error('[error-boundary]', this.props.what ?? 'render', error, info.componentStack)
  }

  componentDidUpdate(prev: BoundaryProps) {
    if (this.state.error && prev.resetKey !== this.props.resetKey) this.setState({ error: null })
  }

  reset = () => this.setState({ error: null })

  render() {
    const { error } = this.state
    if (!error) return this.props.children
    if (this.props.fallback) return this.props.fallback(error, this.reset)
    return <ErrorFallback error={error} reset={this.reset} what={this.props.what} variant={this.props.variant} />
  }
}
