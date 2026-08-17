<script lang="ts">
  import { route } from '@/router'
  import { tabFromPath } from '@/lib/route-tabs'
  import FocusInbox from '@/components/inbox/FocusInbox.svelte'
  import ConsoleHome from './home/ConsoleHome.svelte'
  import MailPanel from './home/MailPanel.svelte'
  import AgendaPanel from './home/AgendaPanel.svelte'
  import { HOME_TABS } from './home/home'

  // Home/Today — the seamless landing. Surfaces the human's real job in Talaria's
  // guardrail model (triage · review · unblock), unread mentions, fleet health,
  // and one-tap entries into the work surfaces.
  //
  // `/` is the inbox; `/home/<tab>` is a console tab.
  const tab = $derived(tabFromPath(route.pathname, '/home', HOME_TABS, 'inbox'))
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
