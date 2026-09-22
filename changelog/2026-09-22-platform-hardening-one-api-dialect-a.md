- **Platform hardening: one API dialect, a modular UI kit, a complete SDK.**
  A two-sided audit (all 162 API routes; the whole component tree) worked
  through to zero: a guard module (`requireUser`/`requireAdmin`/
  `requirePerm`/`requireView`/`parseBody`/`actorOf`) replaced ~160
  hand-rolled auth prologues and ~90 body validations; 400s now name the
  first zod issue; audit logging landed on every sensitive mutation that
  lacked it (agent secrets, endpoint key rotations, gateway keys, agent
  edits, cron lifecycle, KB creates, ACL grants); a provider key moved
  out of a GET query string; org-wide views (cost, fleet, inference)
  gained real authz. The kit grew Tabs, Checkbox/Toggle, SectionHeader,
  Segmented, DropdownMenu, SaveButton, CopyButton, Chip modes, and
  EmptyState variants — then the eight worst offender surfaces were swept
  onto them at visual parity. The SDK now exports every primitive and
  type a third-party app needs. Config writes are PUT; actions are POST.
