import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Select } from '@/components/ui/select'
import {
  addChannelAgent,
  addChannelMember,
  deleteChannel,
  removeChannelAgent,
  removeChannelMember,
  type ChannelDetail,
} from '@/lib/channels'
import type { AgentModel } from '@/lib/agents'

// Channel settings: people + agents, and the owner's delete. One modal,
// mirroring Board settings' People/Agents structure.
export function ChannelSettingsModal({
  open,
  onClose,
  channelId,
  channelName,
  detail,
  fleet,
  selfUserId,
  onDeleted,
}: {
  open: boolean
  onClose: () => void
  channelId: string
  channelName: string
  detail: ChannelDetail
  fleet: AgentModel[]
  selfUserId: string | null
  onDeleted: () => void
}) {
  const qc = useQueryClient()
  const [email, setEmail] = useState('')
  const [agentPick, setAgentPick] = useState('')
  const [error, setError] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['channel', channelId] })

  const run = async (fn: () => Promise<void>) => {
    setError(null)
    try {
      await fn()
      await refresh()
    } catch (e) {
      setError((e as Error).message)
    }
  }

  const isOwner = detail.role === 'owner'
  const addable = fleet.filter((a) => !detail.agents.includes(a.id))

  return (
    <Modal open={open} onClose={onClose} title={`#${channelName} settings`}>
      <div className="space-y-5">
        <section>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">People</div>
          <ul className="space-y-1">
            {detail.members.map((m) => (
              <li key={m.userId} className="flex items-center gap-2 text-sm">
                <Avatar name={m.name ?? m.email} className="h-6 w-6 text-xs" />
                <span className="min-w-0 flex-1 truncate">{m.email ?? m.name}</span>
                <span className="text-xs text-muted">{m.role}</span>
                {m.role !== 'owner' && (isOwner || m.userId === selfUserId) && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void run(() => removeChannelMember(channelId, m.userId))}
                  >
                    {m.userId === selfUserId ? 'Leave' : 'Remove'}
                  </Button>
                )}
              </li>
            ))}
          </ul>
          <form
            className="mt-2 flex gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              if (!email.trim()) return
              void run(() => addChannelMember(channelId, email.trim())).then(() => setEmail(''))
            }}
          >
            <Input value={email} onChange={(e) => setEmail(e.target.value)} placeholder="teammate@company.com" />
            <Button type="submit" size="sm" disabled={!email.trim()}>
              Add
            </Button>
          </form>
        </section>

        <section>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">Agents</div>
          {detail.agents.length === 0 && (
            <div className="mb-2 text-xs text-muted">No agents yet — add one, then @mention it to talk.</div>
          )}
          <ul className="space-y-1">
            {detail.agents.map((model) => {
              const a = fleet.find((f) => f.id === model)
              return (
                <li key={model} className="flex items-center gap-2 text-sm">
                  <Avatar name={a?.label ?? model} className="h-6 w-6 text-xs" />
                  <span className="min-w-0 flex-1 truncate">
                    {a?.label ?? model}
                    {a?.role && <span className="ml-1.5 text-xs text-muted">{a.role}</span>}
                  </span>
                  <Button variant="ghost" size="sm" onClick={() => void run(() => removeChannelAgent(channelId, model))}>
                    Remove
                  </Button>
                </li>
              )
            })}
          </ul>
          {addable.length > 0 && (
            <div className="mt-2 flex gap-2">
              <Select value={agentPick} onChange={(e) => setAgentPick(e.target.value)}>
                <option value="">Add an agent…</option>
                {addable.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.label} — {a.role}
                  </option>
                ))}
              </Select>
              <Button
                size="sm"
                disabled={!agentPick}
                onClick={() => void run(() => addChannelAgent(channelId, agentPick)).then(() => setAgentPick(''))}
              >
                Add
              </Button>
            </div>
          )}
        </section>

        {error && (
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {error}
          </div>
        )}

        {isOwner && (
          <section className="border-t border-line-subtle pt-3">
            <Button
              variant="danger"
              size="sm"
              onClick={() => {
                if (!confirm(`Delete #${channelName} and all its messages?`)) return
                void run(() => deleteChannel(channelId)).then(onDeleted)
              }}
            >
              Delete channel
            </Button>
          </section>
        )}
      </div>
    </Modal>
  )
}
