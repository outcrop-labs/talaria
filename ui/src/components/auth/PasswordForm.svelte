<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  const qc = useQueryClient()
  let username = $state('')
  let password = $state('')
  let err = $state<string | null>(null)
  let busy = $state(false)

  const submit = async (e: SubmitEvent) => {
    e.preventDefault()
    busy = true
    err = null
    try {
      // The route's every failure (401 bad credentials, 400 disabled, 429
      // rate-limited) carries its sentence in the error envelope — the door
      // throws it, and errorMessage reads it back out.
      await postJson('/api/auth/password', { username, password })
      await qc.invalidateQueries({ queryKey: ['session'] })
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }
</script>

<form onsubmit={submit} class="flex flex-col gap-3">
  <Input
    placeholder="Username"
    autofocus
    autocomplete="username"
    bind:value={username}
  />
  <Input
    placeholder="Password"
    type="password"
    autocomplete="current-password"
    bind:value={password}
  />
  {#if err}<div transition:slide={{ duration: 150 }} class="font-sans text-sm text-danger">{err}</div>{/if}
  <Button type="submit" disabled={busy || !username || !password}>
    {busy ? 'Signing in' : 'Sign in'}
  </Button>
</form>
