<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import { uploadFile } from '@/lib/attachments'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import { PROFILE_PHOTO_ACCEPT, isUploadedAvatar, validateProfilePhoto } from '@/lib/profile-photo'
  import type { SessionUser } from '@/lib/session'

  // The profile photo (R17, R18). The bytes go through the ordinary uploads
  // door, then `PUT /api/me { avatarUploadId }` claims the upload as the
  // avatar; the session's `picture` comes back as the effective URL, so every
  // avatar in the app follows once ['session'] and ['users'] refetch.
  let { user }: { user: SessionUser | null | undefined } = $props()

  const qc = useQueryClient()
  const autosave = useAutosave()
  let input = $state<HTMLInputElement | null>(null)
  let busy = $state(false)
  let error = $state<string | null>(null)

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['session'] })
    await qc.invalidateQueries({ queryKey: ['users'] })
  }

  const onFile = async (file: File | undefined) => {
    if (!file) return
    error = validateProfilePhoto(file)
    if (error) return
    busy = true
    try {
      const up = await uploadFile(file)
      if ('error' in up) {
        error = up.error
        return
      }
      await putJson<{ ok: true }>('/api/me', { avatarUploadId: up.id })
      await refresh()
      autosave.flash()
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busy = false
      // Choosing the same file twice must fire change again.
      if (input) input.value = ''
    }
  }

  const remove = async () => {
    error = null
    busy = true
    try {
      await putJson<{ ok: true }>('/api/me', { avatarUploadId: null })
      await refresh()
      autosave.flash()
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busy = false
    }
  }
</script>

<SettingsRow label="Photo">
  {#snippet hint()}
    {#if error}
      <span class="text-danger" role="alert">{error}</span>
    {:else}
      PNG, JPEG, WebP or GIF, up to 5 MB.
    {/if}
  {/snippet}
  {#if isUploadedAvatar(user?.picture)}
    <DangerLink onClick={() => void remove()} disabled={busy}>Remove photo</DangerLink>
  {/if}
  <Avatar src={user?.picture} name={user?.name || user?.email} class="h-9 w-9" />
  <Button size="sm" variant="outline" disabled={busy} onclick={() => input?.click()}>
    {busy ? 'Uploading' : 'Upload'}
  </Button>
  <input
    bind:this={input}
    type="file"
    accept={PROFILE_PHOTO_ACCEPT}
    class="hidden"
    onchange={(e) => void onFile(e.currentTarget.files?.[0])}
  />
</SettingsRow>
