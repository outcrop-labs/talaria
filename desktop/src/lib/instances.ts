/** Typed wrappers over the five shell commands (src-tauri/src/commands.rs). */

import { invoke } from '@tauri-apps/api/core'

export interface Instance {
  id: string
  instanceId: string
  label: string
  url: string
  createdAt: number
  lastOpenedAt: number | null
}

export const listInstances = () => invoke<Instance[]>('list_instances')

export const addInstance = (url: string) =>
  invoke<Instance>('add_instance', { url })

export const activateInstance = (id: string) =>
  invoke<void>('activate_instance', { id })

export const showWelcome = () => invoke<void>('show_welcome')

export const removeInstance = (id: string) =>
  invoke<Instance[]>('remove_instance', { id })

/** Strip an invoke rejection down to the message the shell sent. */
export const errorText = (e: unknown): string =>
  String(e).replace(/^Error:\s*/, '').replace(/^"|"$/g, '')
