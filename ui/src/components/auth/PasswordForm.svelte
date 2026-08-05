<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
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
      const res = await fetch('/api/auth/password', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify({ username, password }),
      })
      const data = (await res.json().catch(() => ({}))) as { ok?: boolean; error?: string }
      if (!res.ok || !data.ok) {
        err = data.error ?? 'Sign-in failed'
        return
      }
      await qc.invalidateQueries({ queryKey: ['session'] })
    } catch {
      err = 'Network error'
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
