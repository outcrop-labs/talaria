<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
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
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busy = false
    }
  }
</script>

<div class="mt-5 border-t border-line-subtle pt-4">
  <span class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Profile photo</span>
  <div class="flex items-center gap-4">
    <Avatar src={user?.picture} name={user?.name || user?.email} class="h-16 w-16" />
    <div class="flex items-center gap-3">
      <Button size="sm" variant="outline" disabled={busy} onclick={() => input?.click()}>
        {busy ? 'Uploading' : 'Upload'}
      </Button>
      {#if isUploadedAvatar(user?.picture)}
        <DangerLink onClick={() => void remove()} disabled={busy}>Remove</DangerLink>
      {/if}
    </div>
    <input
      bind:this={input}
      type="file"
      accept={PROFILE_PHOTO_ACCEPT}
      class="hidden"
      onchange={(e) => void onFile(e.currentTarget.files?.[0])}
    />
  </div>
  {#if error}
    <p class="mt-2 text-xs text-danger" role="alert">{error}</p>
  {:else}
    <p class="mt-2 text-xs text-muted">PNG, JPEG, WebP, or GIF up to 5 MB. Shown wherever your avatar appears.</p>
  {/if}
</div>
