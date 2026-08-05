<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import AdminGhFields from './AdminGhFields.svelte'
  import AdminGhStep from './AdminGhStep.svelte'

  /** The full field-by-field setup walkthrough — out of the panel, into a calm
   *  scrollable modal. Mirrors whichever method is selected. */
  let { open, onClose, mode }: { open: boolean; onClose: () => void; mode: 'app' | 'pat' } = $props()

  let tab = $state<'app' | 'pat'>(mode)
  $effect(() => {
    void open
    tab = mode
  })
</script>

<Modal {open} {onClose} width="max-w-2xl" title="Connect GitHub — setup guide">
  <div class="space-y-4">
    <Segmented
      options={[
        { id: 'app', label: 'GitHub App (recommended)' },
        { id: 'pat', label: 'Personal access token' },
      ] as const}
      value={tab}
      onChange={(t) => (tab = t)}
    />
    <div class="max-h-[60vh] space-y-4 overflow-y-auto pr-1">
      {#if tab === 'app'}
        <AdminGhStep n={1} title="Create the App">
          <p class="text-xs text-muted">
            Open{' '}
            <a href="https://github.com/settings/apps/new" target="_blank" rel="noreferrer" class="text-accent hover:underline">
              github.com/settings/apps/new
            </a>{' '}
            — or, if the repos belong to an organization, prefer <span class="text-fg">Org settings → Developer settings → GitHub Apps → New GitHub App</span> so the org owns it outright. GitHub's form is long; here is every field that matters (leave anything unlisted at its default):
          </p>
          <AdminGhFields
            rows={[
              ['GitHub App name', 'Anything unique — e.g. "Talaria Workbench — Yourco".'],
              ['Description', 'Optional.'],
              ['Homepage URL', "Required by GitHub but unused here — your company site or this Talaria instance's address both work."],
              ['Callback URL', 'Leave blank — Talaria never signs users in through this App.'],
              ['Request user authorization / Device flow', 'Leave unchecked.'],
              ['Setup URL / Redirect on update', 'Leave blank / unchecked.'],
              ['Webhook → Active', 'UNCHECK it. Talaria calls GitHub directly, so no webhook — unchecking also removes the required-URL field.'],
              ['Repository permissions', 'Contents: Read and write · Pull requests: Read and write. (Metadata: Read-only is set automatically.)'],
              ['Organization permissions', 'None needed. (Optional: Administration — Read and write, only if you later want agents to request NEW repos with human approval.)'],
              ['Subscribe to events', 'None.'],
              ['Where can this app be installed?', 'If the repos live in an ORGANIZATION and the app is created under your personal account, choose "Any account" — "Only on this account" locks installs to the owner. (Already created it? App settings → Advanced → "Make public" flips this; public only means installable elsewhere, nothing is exposed.) Org-owned apps can stay "Only on this account".'],
            ]}
          />
        </AdminGhStep>
        <AdminGhStep n={2} title="Collect the credentials">
          <p class="text-xs text-muted">
            After "Create GitHub App": the <span class="text-fg">App ID</span> is at the top of the app's settings page (About section). Then scroll to{' '}
            <span class="text-fg">Private keys → Generate a private key</span> — it downloads a <span class="text-fg">.pem</span> file; open it in any text editor and paste the whole thing into the panel, BEGIN/END lines included. Both are stored encrypted.
          </p>
        </AdminGhStep>
        <AdminGhStep n={3} title="Install it on your repos">
          <p class="text-xs text-muted">
            On the app's settings page choose <span class="text-fg">Install App</span> in the sidebar → pick the account or organization (org installs by non-owners become a request an org owner approves) →{' '}
            <span class="text-fg">"Only select repositories"</span> → choose the repos agents will work (you can add more any time). Then pick that installation in the panel's "Installed on" selector.
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
              ['Token name', 'Anything recognizable — e.g. "Talaria Workbench".'],
              ['Resource owner', 'The account or organization that owns the repos your agents will work.'],
              ['Expiration', 'Your policy — you re-paste a fresh token when it rotates.'],
              ['Repository access', '"Only select repositories" → pick the repos agents may touch. You can add more later.'],
              ['Permissions → Repository', 'Contents: Read and write · Pull requests: Read and write. (Metadata: Read-only is added automatically.)'],
              ['Everything else', 'Leave at its default.'],
            ]}
          />
        </AdminGhStep>
        <AdminGhStep n={2} title="Paste it in the panel">
          <p class="text-xs text-muted">Stored encrypted; never shown again. The status line confirms who it acts as.</p>
        </AdminGhStep>
      {/if}
    </div>
    <div class="flex justify-end">
      <Button size="sm" variant="outline" onclick={onClose}>
        Done
      </Button>
    </div>
  </div>
</Modal>
