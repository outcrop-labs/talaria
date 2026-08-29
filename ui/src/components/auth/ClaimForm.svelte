<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  const qc = useQueryClient()
  let email = $state('')
  let password = $state('')
  let name = $state('')
  let err = $state<string | null>(null)
  let busy = $state(false)

  const submit = async (e: SubmitEvent) => {
    e.preventDefault()
    busy = true
    err = null
    try {
      await postJson('/api/auth/claim', {
        email,
        password,
        ...(name.trim() ? { name: name.trim() } : {}),
      })
      // The claim flips both facts this screen depends on: there is now a
      // session, and the instance is no longer claimable.
      await qc.invalidateQueries({ queryKey: ['session'] })
      await qc.invalidateQueries({ queryKey: ['auth-providers'] })
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }
</script>

<form onsubmit={submit} class="flex flex-col gap-3">
  <Input placeholder="Your email" type="email" autofocus autocomplete="username" bind:value={email} />
  <Input placeholder="Your name (optional)" autocomplete="name" bind:value={name} />
  <Input
    placeholder="Password (min 8)"
    type="password"
    autocomplete="new-password"
    bind:value={password}
  />
  {#if err}<div transition:slide={{ duration: 150 }} class="font-sans text-sm text-danger">{err}</div>{/if}
  <Button type="submit" disabled={busy || !email || password.length < 8}>
    {busy ? 'Claiming' : 'Claim this instance'}
  </Button>
</form>
