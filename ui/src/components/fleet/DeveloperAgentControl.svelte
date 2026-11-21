<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import DeveloperSwitchRow from '@/components/github-access/DeveloperSwitchRow.svelte'
  import { useGithubPermissions } from '@/components/github-access/queries'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { permsHint } from '@/lib/github-access'
  import { slide } from '@/lib/motion'
  import CodingAccounts from './CodingAccounts.svelte'

  // The one developer setting, on the agent's Overview. On sets the agent up
  // end to end (the dev sandbox, Oh My Pi as its coding harness, and the
  // Workbench tools start_job / finish_job) and rolls the agent so it lands.
  // Which repos it may touch and what its GitHub token carries live together
  // in the agent's GitHub access section, which the row under the switch
  // opens; the switch never grants either.
  let { def, canManage, onGithub }: { def: AgentDef; canManage: boolean; onGithub?: () => void } = $props()

  // The same read, under the same key, the GitHub access section makes.
  const perms = useGithubPermissions(() => def.id, () => def.developer)
</script>

<DeveloperSwitchRow {def} {canManage} />
{#if def.developer}
  <div transition:slide={{ duration: 150 }}>
    <SettingsRow
      label="Repositories and GitHub access"
      hint={perms.data ? permsHint(perms.data) : perms.isError ? 'Could not read its GitHub access' : undefined}
      onOpen={onGithub}
      openLabel="Open its repositories and GitHub access"
    />
  </div>
  {#if canManage}
    <!-- Coding accounts answer the next question in the same breath: it may
         touch these repos, and it codes on this plan. The panel renders
         nothing, its padded wrapper included, when the feature is off (its
         route 404s, which the panel treats as "absent" rather than an
         error), so the group draws no empty band. -->
    <CodingAccounts agentId={def.id} class="px-5 py-4" />
  {/if}
{/if}
