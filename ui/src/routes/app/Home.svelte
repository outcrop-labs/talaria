<script lang="ts">
  import { searchParams } from 'sv-router'
  import FocusInbox from '@/components/inbox/FocusInbox.svelte'
  import ConsoleHome from './home/ConsoleHome.svelte'
  import MailPanel from './home/MailPanel.svelte'
  import AgendaPanel from './home/AgendaPanel.svelte'
  import { HOME_TABS, type HomeTab } from './home/home'

  // Home/Today — the seamless landing. Surfaces the human's real job in Talaria's
  // guardrail model (triage · review · unblock), unread mentions, fleet health,
  // and one-tap entries into the work surfaces.
  //
  // /?tab=boards deep-links a console tab (the old route's validateSearch).
  const tab = $derived.by((): HomeTab => HOME_TABS.find((v) => v === searchParams.get('tab')) ?? 'inbox')
</script>

{#snippet mail()}
  <MailPanel />
{/snippet}

{#snippet agenda()}
  <AgendaPanel />
{/snippet}

{#if tab === 'inbox'}
  <FocusInbox {mail} {agenda} />
{:else}
  <ConsoleHome {tab} />
{/if}
