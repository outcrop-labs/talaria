<script lang="ts">
  import { route } from '@/router'
  import { tabFromPath } from '@/lib/route-tabs'
  import DailyBrief from '@/components/brief/DailyBrief.svelte'
  import ConsoleHome from './home/ConsoleHome.svelte'
  import MailPanel from './home/MailPanel.svelte'
  import AgendaPanel from './home/AgendaPanel.svelte'
  import { HOME_TABS } from './home/home'

  // Home/Today — the seamless landing.
  //
  // `/` IS THE DAILY BRIEF NOW, where it used to be the focus queue. The queue
  // asked "what is the single next decision", which is the right question once
  // a day; the brief is a document opened before the workday and appended to as
  // the day moves, which is the right question every other time. See the header
  // of components/brief/DailyBrief.svelte.
  //
  // `/home/<tab>` is still a console tab.
  const tab = $derived(tabFromPath(route.pathname, '/home', HOME_TABS, 'inbox'))
</script>

{#snippet mail()}
  <MailPanel />
{/snippet}

{#snippet agenda()}
  <AgendaPanel />
{/snippet}

{#if tab === 'inbox'}
  <DailyBrief {mail} {agenda} />
{:else}
  <ConsoleHome {tab} />
{/if}
