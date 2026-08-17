<script lang="ts" module>
  // ── Unreadable secrets ───────────────────────────────────────────────────────
  // One of the setup bumps — the family doctrine ("nothing blocks; bumps render
  // only on RESOLVED gaps and say what the gap costs") lives in NoModelBump.svelte.

  const DISMISS_KEY = 'talaria.secrets-banner-dismissed'
</script>

<script lang="ts">
  import { p } from '@/router'
  import { slide } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { useSecretHealth } from '@/lib/secrets'

  // App-wide, admin-only, dismissible. Unreadable secrets fail at USE time — a
  // chat that will not start, a Drive sync that stops, an SMTP send that
  // silently does not — so without this an operator learns about it from a
  // confused colleague days later.
  const session = useSession()
  const health = useSecretHealth(() => session.data?.role === 'admin')
  let dismissed = $state<string | null>(null)

  // Read after mount: the server has no localStorage, and rendering the banner
  // on the server only to remove it on hydration is a visible flash.
  $effect(() => {
    dismissed = localStorage.getItem(DISMISS_KEY)
  })

  const data = $derived(health.data)
  const broken = $derived(!!data && (data.root.state === 'unreadable' || data.root.state === 'absent'))

  // The situation, not a boolean. Dismissing "3 unreadable" must not also
  // silence "9 unreadable" next week — a dismissal is "I have seen THIS", and
  // a bigger breakage is a different this.
  const situation = $derived(data ? `${data.root.state}:${data.counts.unreadable}` : null)

  const dismiss = () => {
    if (situation === null) return
    localStorage.setItem(DISMISS_KEY, situation)
    dismissed = situation
  }
</script>

{#if data && (broken || data.counts.unreadable > 0) && dismissed !== situation}
  <div transition:slide={{ duration: 150 }} class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-danger/40 bg-danger/5 px-4 py-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">
      {data.root.state === 'absent' ? 'No encryption root' : 'Secrets unreadable'}
    </span>
    <span class="min-w-0 flex-1 text-xs text-muted">
      {data.root.state === 'absent'
        ? 'TALARIA_SECRET_KEY is not set, so nothing new can be sealed — provider keys and connections cannot be saved.'
        : data.root.state === 'unreadable'
          ? `This instance cannot unwrap its data key, so ${data.counts.unreadable} stored secret${data.counts.unreadable === 1 ? '' : 's'} cannot be read. Restoring the original root secret recovers them.`
          : `${data.counts.unreadable} stored secret${data.counts.unreadable === 1 ? ' is' : 's are'} sealed with a key this instance no longer has.`}
    </span>
    <a
      href={`${p('/admin')}/secrets`}
      class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-danger underline-offset-2 hover:underline"
    >
      Review secrets
    </a>
    <button
      type="button"
      onclick={dismiss}
      class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim transition-colors hover:text-fg"
    >
      <!-- Dismissible on purpose: this is a state to fix, not a modal to fight.
           An operator mid-recovery should not have to argue with the banner. -->
      Dismiss
    </button>
  </div>
{/if}
