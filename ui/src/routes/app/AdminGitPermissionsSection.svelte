<script lang="ts">
  import { createQueries } from '@tanstack/svelte-query'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import { usePermissionRequests } from '@/components/github-access/queries'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import { errorMessage, getJsonOr404 } from '@/lib/fetch-json'
  import { gitHostAgentsKey, unclaimedKeys, type HostAgents } from '@/lib/git-host-agents'
  import { isDecidable } from '@/lib/git-permission-requests'
  import { GITHUB_ACCESS_HREF } from '@/lib/github-access'
  import { useGitHosts } from './git-hosts'

  /** Agent policy › Git hosts' way to each agent's GitHub access, right under
   *  the hosts: the page an admin reaches from here is where requests are
   *  decided and permissions granted (Agent policy › GitHub access). Its
   *  state says why to go there now: a request waiting on an admin first,
   *  else what a GitHub installation accepted that no agent's set names.
   *  A failed requests read is said, never silence. */
  const requests = usePermissionRequests()
  const hostsQuery = useGitHosts()
  const github = $derived((hostsQuery.data?.hosts ?? []).filter((h) => h.kind === 'github'))
  const agents = createQueries(() => ({
    queries: github.map((h) => ({
      queryKey: gitHostAgentsKey(h.id),
      queryFn: () => getJsonOr404<HostAgents>(`/api/workbench/git-hosts/${encodeURIComponent(h.id)}/agents`),
    })),
  }))

  // Only what this admin can decide: a fact-only request (one raised on a
  // board they are not on) is another admin's, so it never says "waiting".
  const pending = $derived((requests.data ?? []).filter(isDecidable).filter((r) => r.status === 'pending').length)
  const unclaimed = $derived([...new Set(agents.flatMap((q) => unclaimedKeys(q.data)))])
  const pointer = $derived.by((): { word: string; title?: string } | null => {
    if (requests.isError && requests.data === undefined) return { word: 'Could not read requests', title: errorMessage(requests.error) }
    if (pending > 0) return { word: `${pending} request${pending === 1 ? '' : 's'} waiting` }
    if (unclaimed.length > 0) {
      const more = unclaimed.length > 2 ? ` +${unclaimed.length - 2}` : ''
      return {
        word: `Granted to no agent: ${unclaimed.slice(0, 2).join(', ')}${more}`,
        title: `Accepted by an installation, granted to no agent: ${unclaimed.join(', ')}`,
      }
    }
    return null
  })
</script>

<SettingsGroup data-testid="git-permission-pointer">
  <SettingsRow label="Agents' GitHub access" hint="Each agent's repositories, permissions, tokens and requests." openHref={GITHUB_ACCESS_HREF}>
    {#snippet status()}
      <!-- Keys can be long: the word may wrap on a phone rather than push the page sideways. -->
      {#if pointer}<RowStatus status="warn" title={pointer.title} class="min-w-0 whitespace-normal">{pointer.word}</RowStatus>{/if}
    {/snippet}
  </SettingsRow>
</SettingsGroup>
