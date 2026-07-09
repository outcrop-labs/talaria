import { createFileRoute, redirect } from '@tanstack/react-router'

// Channels live inside Comms now (alongside Relays and DMs).
export const Route = createFileRoute('/_app/channels')({
  beforeLoad: () => {
    throw redirect({ to: '/comms' })
  },
})
