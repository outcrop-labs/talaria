<script lang="ts">
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import AdminGhFields from './AdminGhFields.svelte'
  import AdminGhStep from './AdminGhStep.svelte'

  /** The full field-by-field setup walkthrough — out of the panel, into a calm
   *  scrollable modal. Mirrors whichever method is selected.
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
</script>

<Modal {open} {onClose} width="max-w-2xl" title="Connect GitHub: setup guide">
  <div class="space-y-4">
    <Segmented
      options={[
        { id: 'app', label: 'GitHub App (recommended)' },
        { id: 'pat', label: 'Personal access token' },
      ] as const}
      value={tab}
      onChange={(t) => (tab = t)}
    />
    <!-- Tab-pane grammar, in a modal whose height follows the guide: the modal
         glides between the two methods' heights (AutoHeight), the steps rise
         in with a stagger — this pane is a stack of sections. -->
    <AutoHeight>
      <div class="max-h-[60vh] overflow-y-auto pr-1">
        {#key tab}
        <div in:fly={{ y: 6, duration: 200 }} use:staggerIn class="space-y-4">
      {#if tab === 'app'}
        <p class="rounded-md border border-line-subtle bg-raised/40 px-3 py-2 text-xs text-muted">
          <span class="text-fg">The App is the ceiling, not the grant.</span> Give it every permission any agent here should ever use. Talaria mints each token for one repository and narrows it to what an admin granted that agent on this host, so nothing the App holds reaches an agent by itself — and nothing the App lacks can be granted.
        </p>
        <AdminGhStep n={1} title="Create the App">
          <p class="text-xs text-muted">
            Open{' '}
            <a href="https://github.com/settings/apps/new" target="_blank" rel="noreferrer" class="text-accent hover:underline">
              github.com/settings/apps/new
            </a>, or
            if the repos belong to an organization, prefer <span class="text-fg">Org settings → Developer settings → GitHub Apps → New GitHub App</span> so the org owns it outright. GitHub's form is long; here is every field that matters (leave anything unlisted at its default):
          </p>
          <AdminGhFields
            rows={[
              ['GitHub App name', 'Anything unique, e.g. "Talaria Workbench (Yourco)".'],
              ['Description', 'Optional.'],
              ['Homepage URL', "Required by GitHub but unused here: your company site or this Talaria instance's address both work."],
              ['Callback URL', 'Leave blank. Talaria never signs users in through this App.'],
              ['Request user authorization / Device flow', 'Leave unchecked.'],
              ['Setup URL / Redirect on update', 'Leave blank / unchecked.'],
              ['Webhook → Active', 'UNCHECK it. Talaria calls GitHub directly, so no webhook; unchecking also removes the required-URL field.'],
              ['Repository permissions: always', 'Contents: Read and write (clone, push the job branch) · Pull requests: Read and write (Talaria opens the pull request) · Checks: Read-only and Commit statuses: Read-only (a pull request\'s CI on a private repo) · Metadata: Read-only (set automatically).'],
              ['Repository permissions: what agents may use', 'Anything else an agent here should ever be able to use: Workflows (to change .github/workflows), Actions, Secrets, Variables, Commit statuses write. Granting it here gives it to no agent; an admin still picks it per agent per host.'],
              ['Repository → Administration', 'Only if agents may request NEW repos (a person approves each): Talaria spends it itself to protect the new repo\'s default branch, and never puts it on an agent\'s token.'],
              ['Organization permissions', 'None needed. (Administration: Read and write only for the same repo requests: creating a repo in the org.)'],
              ['Subscribe to events', 'None.'],
              ['Where can this app be installed?', 'If the repos live in an ORGANIZATION and the app is created under your personal account, choose "Any account". "Only on this account" locks installs to the owner. (Already created it? App settings → Advanced → "Make public" flips this; public only means installable elsewhere, nothing is exposed.) Org-owned apps can stay "Only on this account".'],
            ]}
          />
        </AdminGhStep>
        <AdminGhStep n={2} title="Collect the credentials">
          <p class="text-xs text-muted">
            After "Create GitHub App": the <span class="text-fg">App ID</span> is at the top of the app's settings page (About section). Then scroll to{' '}
            <span class="text-fg">Private keys → Generate a private key</span>. It downloads a <span class="text-fg">.pem</span> file; open it in any text editor and paste the whole thing into the panel, BEGIN/END lines included. Both are stored encrypted.
          </p>
        </AdminGhStep>
        <AdminGhStep n={3} title="Install it on your repos">
          <p class="text-xs text-muted">
            On the app's settings page choose <span class="text-fg">Install App</span> in the sidebar → pick the account or organization (org installs by non-owners become a request an org owner approves) →{' '}
            <span class="text-fg">"Only select repositories"</span> → choose the repos agents will work (you can add more any time). Then pick that installation in the panel's "Installed on" chips.
          </p>
        </AdminGhStep>
        <AdminGhStep n={4} title="Changing the App's permissions later">
          <p class="text-xs text-muted">
            Add a permission to the App and GitHub asks an owner of every account it is installed on to accept it. Until they do, the host card's <span class="text-fg">Permissions</span> lists it as <span class="text-fg">waiting for org-owner approval</span>, with the link where they accept it, an alert says so, and tokens are minted without it. Talaria reads the App and each installation again every ten minutes; <span class="text-fg">Refresh</span> on the card reads them now.
          </p>
        </AdminGhStep>
      {:else}
        <AdminGhStep n={1} title="Create a fine-grained token">
          <p class="text-xs text-muted">
            Open{' '}
            <a href="https://github.com/settings/personal-access-tokens/new" target="_blank" rel="noreferrer" class="text-accent hover:underline">
              github.com/settings/personal-access-tokens/new
            </a>{' '}
            and fill the form like this:
          </p>
          <AdminGhFields
            rows={[
              ['Token name', 'Anything recognizable, e.g. "Talaria Workbench".'],
              ['Resource owner', 'The account or organization that owns the repos your agents will work.'],
              ['Expiration', 'Your call: re-paste a fresh token when it rotates.'],
              ['Repository access', '"Only select repositories" → pick the repos agents may touch. You can add more later.'],
              ['Permissions → Repository', 'Contents: Read and write · Pull requests: Read and write · Checks: Read-only · Commit statuses: Read-only. (Metadata: Read-only is added automatically.)'],
              ['Everything else', 'Leave at its default.'],
            ]}
          />
          <p class="text-xs text-muted">
            A token cannot be narrowed: every agent granted a repo on this host can spend all of it. The GitHub App mints a token per repository, narrowed per agent; prefer it.
          </p>
        </AdminGhStep>
        <AdminGhStep n={2} title="Paste it in the panel">
          <p class="text-xs text-muted">Stored encrypted; never shown again. The status line confirms who it acts as.</p>
        </AdminGhStep>
      {/if}
        </div>
        {/key}
      </div>
    </AutoHeight>
    <div class="flex justify-end">
      <Button size="sm" variant="outline" onclick={onClose}>
        Done
      </Button>
    </div>
  </div>
</Modal>
