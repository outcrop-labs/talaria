- **WYSIWYG everywhere (#46).** The remaining prose fields that still edited
  as raw textareas now use the shared rich editor: template skeleton bodies
  (the sections agents fill — now edited the way tickets render them),
  the create-agent soul draft (parity with the post-creation editor),
  assistant personality in both the wizard and the inline settings field,
  and the plan modal's AI-drafted ticket descriptions. One long-standing
  inconsistency fixed: YOUR chat bubbles now render markdown like every
  other message surface (they were the one bubble showing raw text). The
  plan document gains the fullscreen toggle (Esc exits) that artifacts and
  KB docs already had. Deliberate leaves, now written into
  docs/UI-CONVENTIONS.md → Editors: chat/channel composers stay plain
  textareas (Enter-to-send + caret mentions beat a toolbar; porting
  mentions to a TipTap suggestion is its own thread), and machine/prompt
  text (YAML, raw HTML, AI instructions, template guidance) stays mono
  Textarea on purpose.

### Added
