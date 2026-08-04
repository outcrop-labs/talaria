import assert from 'node:assert/strict'
import test from 'node:test'
import {
  DEFAULT_INBOX_PANEL_WIDTH,
  MAX_INBOX_PANEL_WIDTH,
  MIN_INBOX_PANEL_WIDTH,
  clampInboxPanelWidth,
  shouldCollapseInboxPanel,
} from './inbox-panel-size'
import { shouldAttachInboxDecision } from './inbox-focus-surface'

test('panel width stays within the adjustable desktop range', () => {
  assert.equal(clampInboxPanelWidth(280), MIN_INBOX_PANEL_WIDTH)
  assert.equal(clampInboxPanelWidth(476), 476)
  assert.equal(clampInboxPanelWidth(900), MAX_INBOX_PANEL_WIDTH)
  assert.equal(clampInboxPanelWidth(Number.NaN), DEFAULT_INBOX_PANEL_WIDTH)
})

test('dragging past the left detent collapses into the conversation rail', () => {
  assert.equal(shouldCollapseInboxPanel(260), true)
  assert.equal(shouldCollapseInboxPanel(319), true)
  assert.equal(shouldCollapseInboxPanel(MIN_INBOX_PANEL_WIDTH), false)
  assert.equal(shouldCollapseInboxPanel(DEFAULT_INBOX_PANEL_WIDTH), false)
})

test('only the canonical Inbox surface attaches the active decision', () => {
  assert.equal(shouldAttachInboxDecision('/', undefined), true)
  assert.equal(shouldAttachInboxDecision('/', 'inbox'), true)
  assert.equal(shouldAttachInboxDecision('/', 'boards'), false)
  assert.equal(shouldAttachInboxDecision('/comms', undefined), false)
  assert.equal(shouldAttachInboxDecision('/plan', undefined), false)
})
