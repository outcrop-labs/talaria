import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSession } from '@/lib/session'

export const Route = createFileRoute('/_app/settings')({
  component: SettingsPage,
})

// Personal settings. Just the profile for now; more sections land here later.
function SettingsPage() {
  const qc = useQueryClient()
  const { data: user } = useSession()
  const [name, setName] = useState('')
  const [saved, setSaved] = useState(false)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (user) setName(user.name ?? '')
  }, [user])

  const save = async () => {
    const n = name.trim()
    if (!n || n === user?.name) return
    setBusy(true)
    try {
      const r = await fetch('/api/profile', {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name: n }),
      })
      if (r.ok) {
        await qc.invalidateQueries({ queryKey: ['session'] })
        await qc.invalidateQueries({ queryKey: ['users'] })
        setSaved(true)
        setTimeout(() => setSaved(false), 1500)
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mx-auto w-full max-w-lg p-8">
      <h1 className="mercury-text mb-4 text-lg font-semibold">Settings</h1>
      <section className="mercury-panel rounded-2xl p-5">
        <div className="mb-3 flex items-center gap-3">
          <Avatar src={user?.picture} name={name || user?.email} className="h-10 w-10" />
          <div className="min-w-0">
            <div className="truncate text-sm font-medium text-fg">{name || user?.email}</div>
            <div className="truncate text-xs text-muted">{user?.email}</div>
          </div>
        </div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Display name</label>
        <div className="flex items-center gap-2">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && void save()}
            placeholder="How teammates and agents see you"
          />
          <Button onClick={() => void save()} disabled={busy || !name.trim() || name.trim() === user?.name}>
            Save
          </Button>
        </div>
        {saved && <div className="mt-2 text-xs text-[color:var(--theme-success)]">Saved</div>}
      </section>
    </div>
  )
}
