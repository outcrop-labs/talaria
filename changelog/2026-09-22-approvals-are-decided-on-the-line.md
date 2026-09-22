- **Approvals are decided on the line, not by asking around them.** A
  pending outbound action — an email or an event the assistant drafted —
  appeared as a p0 "Needs you" line that went nowhere: its href was a
  placeholder `/`, and no page in the app listed pending actions at all, so
  the only way to act was a conversation with the assistant. The line now
  carries its own decision block, quoting the exact outbound payload (who
  receives it, what it says, who drafted it) above Approve/Reject buttons —
  the same contract the drafted-reply block keeps, which is why neither
  carries a modal. The buttons ride the existing
  `/api/integrations/google/pending/{id}` decide route; the sweep reads the
  pending row's decided status and closes the line as APPROVED or REJECTED
  rather than a generic DONE. Verified on the running stack: the line
  renders with its payload, a reject closes it as REJECTED on the next
  read.
