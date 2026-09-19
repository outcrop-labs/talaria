// Task/board constants — mirror of ui/src/lib/task-const.ts, so the client
// and this crate agree on the ladders. (TASK_STATUSES/OFF_BOARD_STATUSES
// stay in statuses.rs, their server-side home, beside the resolvers that
// use them.)

/// The ticket priority ladder.
pub const PRIORITIES: &[&str] = &["low", "medium", "high", "urgent"];

/// T-shirt effort sizes.
pub const EFFORTS: &[&str] = &["xs", "s", "m", "l", "xl"];

/// The board color palette a ticket may carry.
pub const TICKET_COLORS: &[&str] = &[
    "slate", "bronze", "green", "amber", "red", "blue", "purple", "teal", "pink", "orange", "lime",
    "cyan", "indigo", "magenta", "olive", "brown",
];
