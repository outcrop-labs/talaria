<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import AdminGhFields from './AdminGhFields.svelte'
  import AdminGhStep from './AdminGhStep.svelte'

  /** The field-by-field setup walkthrough for a GitHub connection. Opens on
   *  whichever method the connection dialog has selected.
   *
   *  THE APP IS THE CEILING, NOT THE GRANT. Give the App every permission any
   *  agent should ever use: Talaria mints each token for one repository and
   *  narrows it to what an admin granted that agent on this host, so a
   *  permission the App holds reaches no agent by itself. What the App lacks,
   *  no grant can give. */
  let { open, onClose, mode }: { open: boolean; onClose: () => void; mode: 'app' | 'pat' } = $props()

  // svelte-ignore state_referenced_locally -- reason: the reopen $effect below re-seeds it from the live prop
  let tab = $state<'app' | 'pat'>(mode)
  $effect(() => {
    void open
    tab = mode
  })

  const text = 'font-sans text-[13px] leading-5 text-muted'
  const link = 'text-accent underline-offset-2 hover:underline'
</script>

<Modal {open} {onClose} width="max-w-2xl" title="GitHub setup guide">
  <div class="space-y-6">
    <Segmented
      options={[
        { id: 'app', label: 'GitHub App' },
        { id: 'pat', label: 'Access token' },
      ] as const}
      value={tab}
      onChange={(t) => (tab = t)}
    />
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} use:staggerIn class="space-y-6">
        {#if tab === 'app'}
          <p class={text}>
            <span class="text-fg">The app is the ceiling, not the grant.</span> Give it every permission any agent here should ever use.
            Talaria mints each token for one repository and narrows it to what an admin granted that agent on this host, so nothing the app
            holds reaches an agent by itself, and nothing the app lacks can be granted.
          </p>
          <AdminGhStep n={1} title="Create the app">
            <p class={text}>
              Open
              <a href="https://github.com/settings/apps/new" target="_blank" rel="noreferrer" class={link}>github.com/settings/apps/new</a>.
              If an organization owns the repositories, create it from the organization's developer settings instead. Leave any field
              not listed here as it is.
            </p>
            <AdminGhFields
              rows={[
                ['GitHub App name', 'Anything unique, such as "Talaria Workbench".'],
                ['Homepage URL', 'Required but unused. Your company site works.'],
                ['Callback URL', 'Leave blank.'],
                ['Request user authorization', 'Leave unchecked, and Device flow too.'],
                ['Webhook', 'Uncheck Active. Talaria calls GitHub directly.'],
                ['Repository permissions', "Contents and Pull requests, both Read and write. Checks and Commit statuses, Read-only, for a pull request's CI on a private repository."],
                ['Other repository permissions', 'Anything an agent here should ever use, such as Workflows, Actions, Secrets or Variables. It reaches no agent until an admin grants it to that agent on this host.'],
                ['Repository administration', "Only if agents will request new repositories, which a person approves. Talaria uses it to protect the new repository's default branch and never puts it on an agent's token."],
                ['Organization permissions', 'None. Add Administration, Read and write, only if agents will request new repositories.'],
                ['Subscribe to events', 'None.'],
                ['Where can it be installed', 'Any account, if the app is under your personal account and an organization owns the repositories. Advanced, Make public changes it later.'],
              ]}
            />
          </AdminGhStep>
          <AdminGhStep n={2} title="Copy the App ID and a private key">
            <p class={text}>
              The App ID is at the top of the app's settings page. Under Private keys, generate a key, open the downloaded .pem file in a
              text editor, and paste all of it into the connection dialog.
            </p>
          </AdminGhStep>
          <AdminGhStep n={3} title="Install it on your repositories">
            <p class={text}>
              Choose Install App in the app's sidebar, pick the account, then Only select repositories. Back in the connection dialog,
              turn that installation on.
            </p>
          </AdminGhStep>
          <AdminGhStep n={4} title="Change the app's permissions later">
            <p class={text}>
              Add a permission to the app and GitHub asks an owner of every account it is installed on to accept it. Until they do, the
              host's row says Approval waiting, its dialog lists the permission as waiting with the link where they accept it, an alert says
              so, and tokens are minted without it. Talaria reads the app and each installation every ten minutes, and Refresh in the host's
              dialog reads them now.
            </p>
          </AdminGhStep>
        {:else}
          <AdminGhStep n={1} title="Create a fine-grained token">
            <p class={text}>
              Open
              <a href="https://github.com/settings/personal-access-tokens/new" target="_blank" rel="noreferrer" class={link}>
                github.com/settings/personal-access-tokens/new
              </a>
              and fill it in like this.
            </p>
            <AdminGhFields
              rows={[
                ['Token name', 'Anything you will recognize, such as "Talaria Workbench".'],
                ['Resource owner', 'The account or organization that owns the repositories.'],
                ['Expiration', 'Your choice. Paste a fresh token when it rotates.'],
                ['Repository access', 'Only select repositories, then the ones agents may touch.'],
                ['Repository permissions', 'Contents and Pull requests, both Read and write. Checks and Commit statuses, Read-only.'],
              ]}
            />
            <p class={text}>
              A token cannot be narrowed: every agent granted a repository on this host can spend all of it. The GitHub App mints a token per
              repository, narrowed per agent, so prefer it.
            </p>
          </AdminGhStep>
          <AdminGhStep n={2} title="Paste it into the connection dialog">
            <p class={text}>It is stored encrypted and never shown again. The host's row then says who it acts as.</p>
          </AdminGhStep>
        {/if}
      </div>
    {/key}
  </div>
  {#snippet footer()}
    <div class="flex justify-end">
      <Button size="sm" variant="outline" onclick={onClose}>Done</Button>
    </div>
  {/snippet}
</Modal>
