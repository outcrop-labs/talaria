import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Combobox } from '@/components/ui/combobox'
import { Modal } from '@/components/ui/modal'
import { UserPicker } from '@/components/app/user-picker'
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
  const agentOptions = fleet.map((a) => ({ value: a.id, label: a.label, sub: a.role }))

  // The combobox toggles one agent per change — diff against the channel's
  // current set and apply immediately (membership is instant, like People).
  const setAgents = (next: string[]) => {
    const cur = new Set(detail.agents)
    const nextSet = new Set(next)
    const added = next.find((m) => !cur.has(m))
    const removed = detail.agents.find((m) => !nextSet.has(m))
    if (added) void run(() => addChannelAgent(channelId, added))
    if (removed) void run(() => removeChannelAgent(channelId, removed))
  }

  return (
    <Modal open={open} onClose={onClose} title={`#${channelName} settings`}>
      <div className="space-y-5">
        <section>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">People</div>
          <ul className="space-y-1">
            {detail.members.map((m) => (
              <li key={m.userId} className="flex items-center gap-2 text-sm">
                <Avatar name={m.name ?? m.email} className="h-6 w-6 text-xs" />
                <span className="min-w-0 flex-1 truncate">
                  {m.name ?? m.email}
                  {m.name && m.email && <span className="ml-1.5 text-xs text-muted">{m.email}</span>}
                </span>
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
          <UserPicker
            className="mt-2"
            exclude={detail.members.map((m) => m.userId)}
            onPick={(u) => {
              if (u.email) void run(() => addChannelMember(channelId, u.email!))
            }}
          />
        </section>

        <section>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">Agents</div>
          <p className="mb-2 text-xs text-muted">@mention an agent in the channel to bring it into the conversation.</p>
          <Combobox
            options={agentOptions}
            selected={detail.agents}
            onChange={setAgents}
            multiple
            placeholder="Select agents…"
          />
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
