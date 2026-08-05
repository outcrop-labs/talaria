<script lang="ts">
  // ── No email transport ───────────────────────────────────────────────────────
  // One of the setup bumps — the family doctrine ("nothing blocks; bumps render
  // only on RESOLVED gaps and say what the gap costs") lives in NoModelBump.svelte.

  import { useSession } from '@/lib/session'
  import { useSecretHealth } from '@/lib/secrets'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'

  // Invites are sent by email. Without a transport an admin can create one and
  // watch it go nowhere, which looks like a bug in invites.
  let { class: className }: { class?: string } = $props()

  const session = useSession()
  // Read from the inventory rather than /api/admin/email, so this bump and the
  // Secrets page can never disagree about whether email is set up.
  const health = useSecretHealth(() => session.data?.role === 'admin')
  const data = $derived(health.data) // no answer yet, or the read failed — not a gap
  const email = $derived(data?.rows.find((r) => r.id.startsWith('setting:email_config:')))
</script>

{#if data && email?.state !== 'ok'}
  <p transition:slide={{ duration: 150 }} class={cn('text-xs text-warning', className)}>
    {email?.state === 'unreadable'
      ? 'The stored email credential cannot be read, so invites will not be delivered — you can still copy an invite link and send it yourself. Replace it under Organization.'
      : 'No email transport is configured, so invites cannot be delivered — you can still copy an invite link and send it yourself. Set one up under Organization.'}
  </p>
{/if}
