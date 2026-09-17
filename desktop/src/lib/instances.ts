/** Typed wrappers over the shell commands (src-tauri/src/commands.rs). */

import { invoke } from '@tauri-apps/api/core'

export interface Instance {
  id: string
  instanceId: string
  label: string
  url: string
  createdAt: number
  lastOpenedAt: number | null
}

export type TitlebarMode = 'themed' | 'os' | 'none'

export interface DesktopSettings {
  titlebar: TitlebarMode
}

export type DesktopWindowAction = 'minimize' | 'toggleMaximize' | 'close' | 'startDragging'

export const listInstances = () => invoke<Instance[]>('list_instances')

export const addInstance = (url: string) =>
  invoke<Instance>('add_instance', { url })

export const activateInstance = (id: string) =>
  invoke<void>('activate_instance', { id })

export const showWelcome = () => invoke<void>('show_welcome')

export const removeInstance = (id: string) =>
  invoke<Instance[]>('remove_instance', { id })

export const getDesktopSettings = () => invoke<DesktopSettings>('get_desktop_settings')

export const setTitlebarMode = (mode: TitlebarMode) =>
  invoke<DesktopSettings>('set_titlebar_mode', { mode })

export const desktopWindow = (action: DesktopWindowAction) =>
  invoke<void>('desktop_window', { action })

export interface DesktopUpdate {
  version: string
  notes: string | null
}

export const checkForUpdate = () => invoke<DesktopUpdate | null>('check_for_update')

export const installUpdate = () => invoke<void>('install_update')

/** Strip an invoke rejection down to the message the shell sent. */
export const errorText = (e: unknown): string =>
  String(e).replace(/^Error:\s*/, '').replace(/^"|"$/g, '')
