<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'

  // The assistant's Google Workspace connection — THE grant that makes
  // "manage my mail and calendar" real. Rendered inside Settings › Assistant
  // because that is where the owner looks at their assistant and asks what it
  // can do; the same connection also powers doc export on the Connections tab.
  //
  // The card is honest about the loop: reads are live, anything OUTBOUND
  // (send, invite) is drafted by the assistant and waits in the Inbox until
  // the owner approves — that is the whole safety story, and it belongs next
  // to the connect button rather than in a footnote.
  interface GoogleStatus {
    available: boolean
    connected: boolean
    email: string | null
    connectedAt: string | null
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['integration-google'],
    queryFn: (): Promise<GoogleStatus> => getJson<GoogleStatus>('/api/integrations/google'),
  }))
  const data = $derived(query.data)

  let error = $state<string | null>(null)

  const disconnect = async () => {
    if (
      !(await confirm({
        title: 'Disconnect Google',
        message: 'Disconnect your Google Workspace account? Your assistant loses mail, calendar, and Drive access until you reconnect.',
        confirmLabel: 'Disconnect',
        danger: true,
      }))
    )
      return
    error = null
    const r = await fetch('/api/integrations/google', { method: 'DELETE', credentials: 'same-origin' })
    if (!r.ok) error = 'could not disconnect — try again'
    await qc.invalidateQueries({ queryKey: ['integration-google'] })
  }
</script>

<div class="mt-6 border-t border-line pt-5">
  <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Google Workspace</div>

  {#if query.isPending}
    <!-- Hold the row's shape until the status resolves — "Not connected" is a
         claim about the account, and an in-flight read can't make it. -->
    <div aria-hidden="true" class="flex items-center gap-3 rounded-md border border-line p-4">
      <Skeleton class="h-9 w-9 shrink-0 rounded-md" />
      <div class="min-w-0 flex-1 space-y-2">
        <Skeleton class="h-3 w-36 rounded-full" />
        <Skeleton class="h-2.5 w-52 rounded-full" />
      </div>
      <Skeleton class="h-9 w-24 rounded-lg" />
    </div>
  {:else if !data}
    <div class="text-xs text-muted">Could not load the Google connection.</div>
  {:else if !data.available}
    <!-- An admin hasn't registered the OAuth client — the fix is theirs, and
         the sentence says whose it is rather than dead-ending. -->
    <div class="text-xs text-muted">
      Google isn’t set up on this server yet — ask an admin to register the Google client in Admin → Org.
    </div>
  {:else}
    <div class="flex items-center gap-3 rounded-md border border-line p-4">
      <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-line bg-raised text-lg">✉️</div>
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-1.5 text-sm font-medium text-fg">
          Gmail · Calendar · Drive
          {#if data.connected}<span aria-hidden="true" class="h-1.5 w-1.5 shrink-0 rounded-full bg-success"></span>{/if}
        </div>
        <div class="truncate font-mono text-[11px] text-muted">
          {data.connected ? `Connected as ${data.email ?? 'your account'}` : 'Not connected'}
        </div>
      </div>
      {#if data.connected}
        <Button variant="ghost" size="sm" class="shrink-0 hover:text-danger" onclick={() => void disconnect()}>
          Disconnect
        </Button>
      {:else}
        <a href="/api/integrations/google/connect" class={cn(buttonClasses({ size: 'sm' }), 'shrink-0')}>
          Connect
        </a>
      {/if}
    </div>
    <p class="mt-2 max-w-prose text-xs text-muted">
      {data.connected
        ? 'Your assistant reads your mail and calendar live. Emails and invites it drafts wait in your Inbox for your approval — nothing sends without you.'
        : 'Connect your account and your assistant can read your mail and calendar, find Drive files, and draft emails and events for you. Every send waits for your approval in the Inbox.'}
    </p>
  {/if}

  {#if error}
    <div transition:slide={{ duration: 150 }} class="mt-2 text-xs text-danger">{error}</div>
  {/if}
</div>
