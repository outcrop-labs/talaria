import { mount } from 'svelte'
import App from './App.svelte'
import { installDesktopSuspend } from './lib/desktop-suspend'
import './styles.css'

// Inside the desktop shell: let the shell ask whether this page can be
// suspended while it is off screen. Before mount, so the first keystroke is
// seen. A browser gets nothing.
installDesktopSuspend()

const root = document.getElementById('root')
if (!root) throw new Error('missing #root element')

mount(App, { target: root })
