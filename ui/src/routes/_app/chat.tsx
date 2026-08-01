import { createFileRoute, redirect } from '@tanstack/react-router'

// Chat lives inside Comms now (Agents section of the unified sidebar).
// `t=agent` tells Comms to land on the chat workspace — the first agent's
// fresh thread (spec §7 composer + §10 patterns) — instead of the default
// channel selection, so /chat still opens a chat surface.
export const Route = createFileRoute('/_app/chat')({
  beforeLoad: () => {
    throw redirect({ to: '/comms', search: { t: 'agent' } })
  },
})
