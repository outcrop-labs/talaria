- **W10c — tabs, adopted in two of three.** `Segmented` replaces
  `DescriptionSection`'s bespoke pill group (the markup it had WAS Segmented's),
  and `Tabs` replaces `RunDetailModal`'s strip (same frame shape as its sibling
  modal, and the audit already lists "three visually different tab strips" as the
  defect these two commits were converging on). **`BoardSettingsModal` keeps its
  bespoke strip**, with a comment naming the variant `Tabs` needs (fill cells +
  group border): adopting would need two overrides it has no hook for. Two deltas
  the visual pass must judge: `RunDetailModal`'s active cell gains the kit's
  hairline mark, and `Segmented`'s xs cells are ~4px wider each.
