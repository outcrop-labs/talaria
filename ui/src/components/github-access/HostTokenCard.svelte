<script lang="ts">
  import { MediaQuery } from 'svelte/reactivity'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Select from '@/components/ui/Select.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import { checkLines, checkRepos, since, type HostPermissions, type LastToken, type TokenCheck } from '@/lib/agent-github-permissions'
  import { useClock } from '@/lib/clock.svelte'
  import { lastTokenLine } from '@/lib/git-host-agents'

  // What the agent's token on one host carries: Check token (admins) gets the
  // token the agent's next push gets for a repo, the cached one or a fresh
  // mint as the App, recorded with the admin's name, and says what GitHub put
  // on it: what it carries, what GitHub added unasked, and why any key of the
  // set is not on it. The token itself is never shown. Under it, each granted
  // repo's last token, from its credential's audit row.
  let {
    block,
    canCheck,
    checking,
    picked,
    result,
    onPick,
    onCheck,
  }: {
    block: HostPermissions
    canCheck: boolean
    checking: boolean
    /** The repo Check token is for. */
    picked: string
    /** The last answer on this host, and the repo it was for. */
    result: { repo: string; result: TokenCheck } | undefined
    onPick: (repo: string) => void
    onCheck: () => void
  } = $props()

  const clock = useClock()
  const repos = $derived(checkRepos(block))
  const narrow = new MediaQuery('max-width: 639px')
  const TONE: Record<'success' | 'warn' | 'neutral', DotStatus> = { success: 'ok', warn: 'warn', neutral: 'idle' }

  /** A repo's last token in one word: whether it speaks for the set now. */
  function tokenState(t: LastToken | null | undefined): { word: string; status: DotStatus } {
    if (t == null) return { word: 'None', status: 'idle' }
    if (!t.ok) return { word: 'Refused', status: 'warn' }
    if (t.source === 'workspace_secret') return { word: 'Workspace secret', status: 'idle' }
    return t.current === true ? { word: 'Current', status: 'ok' } : { word: 'Earlier set', status: 'idle' }
  }
  const account = $derived(result ? (block.installations ?? []).find((i) => i.id === result.result.installation)?.account : undefined)
</script>

<SettingsSection title={`Token on ${block.name}`}>
  <SettingsGroup>
    {#if canCheck && repos.length > 0}
      <SettingsRow
        label="Check token"
        stacked={narrow.current}
        info="Gets the token this agent's next git push gets for the repository, the cached one or a fresh mint as the App (recorded with your name), and says what GitHub put on it. The token itself is never shown."
      >
        <div class="flex w-full flex-col gap-2 sm:w-auto sm:flex-row sm:items-center">
          {#if repos.length > 1}
            <Select
              size="sm"
              class="w-full sm:w-56"
              aria-label={`Repository to check on ${block.name}`}
              value={picked}
              disabled={checking}
              onchange={(e) => onPick((e.currentTarget as HTMLSelectElement).value)}
            >
              {#each repos as repo (repo)}<option value={repo}>{repo}</option>{/each}
            </Select>
          {/if}
          <Button size="sm" variant="outline" disabled={checking} onclick={onCheck}>{checking ? 'Checking' : 'Check token'}</Button>
        </div>
      </SettingsRow>
      {#if result}
        <ul class="space-y-1.5 px-5 py-4" role="status" data-testid="github-access-check">
          {#each checkLines(result.result, result.repo, block.name, account) as line, i (i)}
            <li class="flex items-start gap-2 font-sans text-[13px] leading-5 text-muted">
              <StatusDot status={TONE[line.tone]} class="mt-[7px]" />
              <span class="min-w-0">{line.text}</span>
            </li>
          {/each}
          {#if result.result.at}<li class="pl-3.5 font-sans text-[13px] leading-5 text-muted">Checked {since(result.result.at, clock.now)}.</li>{/if}
        </ul>
      {/if}
    {/if}
    {#each block.repos as r (r.repo)}
      {@const s = tokenState(r.lastToken)}
      <!-- The row's label is the repo: the line leaves it out. -->
      {@const line = lastTokenLine(r.lastToken ? { ...r.lastToken, repo: null } : r.lastToken, clock.now)}
      <SettingsRow compact hint={r.lastToken === undefined ? undefined : line.text}>
        {#snippet label()}<span class="font-mono text-[13px] font-normal" title={r.repo}>{r.repo}</span>{/snippet}
        {#snippet status()}<RowStatus status={s.status}>{s.word}</RowStatus>{/snippet}
      </SettingsRow>
    {:else}
      <p class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">No repository on {block.name} yet, so no token is minted here.</p>
    {/each}
  </SettingsGroup>
</SettingsSection>
