import assert from 'node:assert/strict'
import { test } from 'vitest'
import { surfaceBrief } from '../server/inbox-focus-policy'
import { ASSISTANT_SURFACE_IDS, assistantSurface, shouldAttachInboxDecision } from './inbox-focus-surface'

test('Home is the Inbox surface only on the tab that is the focus queue', () => {
  assert.equal(assistantSurface('/', undefined).id, 'inbox')
  assert.equal(assistantSurface('/', 'inbox').id, 'inbox')
  assert.equal(assistantSurface('/', 'activity').id, 'home')
  assert.equal(assistantSurface('/', 'activity').label, 'Home')
  // Same rule the decision attachment follows — one source, not two.
  assert.equal(shouldAttachInboxDecision('/', 'activity'), false)
})

test('nested routes resolve to their section, not to the fallback', () => {
  assert.equal(assistantSurface('/boards', undefined).id, 'boards')
  assert.equal(assistantSurface('/boards/board-1', undefined).id, 'boards')
  assert.equal(assistantSurface('/boards/board-1/task-9', undefined).id, 'boards')
  assert.equal(assistantSurface('/x/contacts/manage', undefined).id, 'apps')
  assert.equal(assistantSurface('/kb/space/anything', undefined).id, 'home')
})

test('matching is on a path SEGMENT, not a string prefix', () => {
  // A future '/chatter' route must not inherit Chat's context by spelling.
  assert.equal(assistantSurface('/chatter', undefined).id, 'home')
  assert.equal(assistantSurface('/chat', undefined).id, 'chat')
  assert.equal(assistantSurface('/chat/thread-1', undefined).id, 'chat')
  assert.equal(assistantSurface('/channels', undefined).id, 'comms')
})

test('an unknown route falls back instead of throwing', () => {
  assert.equal(assistantSurface('/nothing-here', undefined).id, 'home')
})

test('every route in the app map resolves to a labelled surface', () => {
  const paths = [
    '/', '/chat', '/comms', '/channels', '/inbox', '/boards', '/plan', '/research',
    '/knowledge', '/artifacts', '/agents', '/fleet', '/studio', '/templates',
    '/models', '/mcp', '/observability', '/apps', '/settings', '/admin',
  ]
  for (const path of paths) {
    const surface = assistantSurface(path, undefined)
    assert.ok(surface.id.length > 0, `${path} has no surface id`)
    assert.ok(surface.label.length > 0, `${path} has no label`)
    assert.ok(ASSISTANT_SURFACE_IDS.includes(surface.id), `${path} produced an unlisted id`)
  }
})

test('every surface the client can send has prose on the server', () => {
  // The two halves live in different files and would drift apart quietly:
  // an id with no brief falls through to "no context", which looks exactly
  // like the bug the surface was added to fix.
  for (const id of ASSISTANT_SURFACE_IDS) {
    assert.ok(surfaceBrief(id), `surface "${id}" has no server brief`)
  }
})

// THE INBOX HAS ITS OWN URL NOW. It used to be `/`, which meant it had no
// address to link to and its nav entry was a root path that is an ancestor of
// every route in the app. It is `/home/inbox`, a route nested under Home.
test('the Inbox surface follows the path tab, not the legacy query tab', () => {
  assert.equal(assistantSurface('/home/inbox', undefined).id, 'inbox')
  assert.equal(assistantSurface('/home', undefined).id, 'inbox') // bare /home renders the default tab
  // THE BUG THIS PINS: `tab` is the legacy `?tab=` param and is undefined on
  // every path-tab route, so reading only that made every Home tab the Inbox.
  assert.equal(assistantSurface('/home/boards', undefined).id, 'home')
  assert.equal(assistantSurface('/home/fleet', undefined).id, 'home')
  assert.equal(shouldAttachInboxDecision('/home/boards', undefined), false)
  assert.equal(shouldAttachInboxDecision('/home/inbox', undefined), true)
})

test('the legacy root still honours ?tab=, because those links are in the world', () => {
  assert.equal(shouldAttachInboxDecision('/', undefined), true)
  assert.equal(shouldAttachInboxDecision('/', 'boards'), false)
})

test('a path that merely starts with /home is not Home', () => {
  assert.equal(assistantSurface('/homework', undefined).id, 'home') // falls through to the default
  assert.equal(shouldAttachInboxDecision('/homework', undefined), false)
})
