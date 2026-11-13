<script lang="ts">
  import { navigate, route } from '@/router'
  import DailyBrief from '@/components/brief/DailyBrief.svelte'
  import MailPanel from './home/MailPanel.svelte'
  import AgendaPanel from './home/AgendaPanel.svelte'

  const onInbox = $derived(route.pathname === '/home' || route.pathname === '/home/inbox')

  $effect(() => {
    if (!onInbox) void navigate('/home/:tab', { params: { tab: 'inbox' }, replace: true })
  })
</script>

{#snippet mail()}
  <MailPanel />
{/snippet}

{#snippet agenda()}
  <AgendaPanel />
{/snippet}

{#if onInbox}
  <DailyBrief {mail} {agenda} />
{/if}
