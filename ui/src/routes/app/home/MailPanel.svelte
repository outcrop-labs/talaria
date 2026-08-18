<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { Mail, Send } from '@lucide/svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import ComposeModal from './ComposeModal.svelte'
  import { useGmail, useGoogleStatus } from './home'

  // Recent Gmail, shown only when the user has connected Google. Compose sends as
  // the user via Gmail.
  const google = useGoogleStatus()
  const connected = $derived(!google.isError && google.data?.connected === true)
  const gmail = useGmail(() => connected)
  let composing = $state(false)

  const messages = $derived(gmail.data?.messages ?? [])

  const fromName = (from: string) => from.replace(/<[^>]*>/, '').replace(/"/g, '').trim() || from
</script>

{#if !google.data && !google.isError}
  <!-- In flight → hold the space with a skeleton; only a RESOLVED
       not-connected state may remove the panel (no pop-in, no dead panel). -->
  <Panel>
    <Skeleton class="mb-4 h-3 w-16 rounded-full" />
    <SkeletonRows rows={4} />
  </Panel>
{:else if !connected}
  <Panel>
    <SectionHeader title="Mail" action="Gmail" />
    <EmptyState variant="inline" title="Connect Gmail to review recent mail here." />
  </Panel>
{:else if !gmail.data && !gmail.isError}
  <Panel>
    <Skeleton class="mb-4 h-3 w-16 rounded-full" />
    <SkeletonRows rows={4} />
  </Panel>
{:else if gmail.isError || gmail.data?.error || !gmail.data}
  <Panel>
    <SectionHeader title="Mail" action="Unavailable" />
    <EmptyState variant="inline" title="Gmail is temporarily unavailable." />
  </Panel>
{:else}
  <Panel>
    <div class="mb-3 flex min-h-6 items-center gap-2">
      <Mail size={14} class="text-ink-dim" />
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Mail</span>
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">Gmail</span>
      <Button variant="ghost" size="xs" class="ml-auto gap-1 text-accent hover:underline" onclick={() => (composing = true)}>
        <Send size={12} /> Compose
      </Button>
    </div>

    {#if messages.length === 0}
      <EmptyState variant="inline" class="py-3" title="No recent mail." />
    {:else}
      <div class="divide-y divide-line">
        {#each messages as m (m.id)}
          <a
            href={`https://mail.google.com/mail/u/0/#all/${m.threadId}`}
            target="_blank"
            rel="noreferrer"
            class="group flex items-center gap-3 py-2 transition-colors hover:bg-card2"
          >
            <span class={cn('w-32 shrink-0 truncate font-sans text-[12px]', m.unread ? 'font-semibold text-fg' : 'text-muted')}>{fromName(m.from)}</span>
            <span class="min-w-0 flex-1 truncate font-sans text-sm">
              <span class={m.unread ? 'font-medium text-fg' : 'text-fg'}>{m.subject}</span>
              <span class="text-muted"> — {m.snippet}</span>
            </span>
            <span class="shrink-0 font-mono text-[11px] text-muted">{m.date ? relativeTime(m.date) : ''}</span>
          </a>
        {/each}
      </div>
    {/if}

    {#if composing}<ComposeModal onClose={() => (composing = false)} />{/if}
  </Panel>
{/if}
