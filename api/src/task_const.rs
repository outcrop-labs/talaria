// Task/board constants — mirror of ui/src/lib/task-const.ts, the shared
// no-deps const module both TS sides import. Only what the port has needed
// so far; the file grows as routes do. (TASK_STATUSES/OFF_BOARD_STATUSES
// stay in statuses.rs, their server-side home, beside the resolvers that
// use them.)

/// The ticket priority ladder (task-const.ts PRIORITIES).
pub const PRIORITIES: &[&str] = &["low", "medium", "high", "urgent"];

/// T-shirt effort sizes (task-const.ts EFFORTS).
pub const EFFORTS: &[&str] = &["xs", "s", "m", "l", "xl"];

/// The board color palette a ticket may carry (task-const.ts TICKET_COLORS).
pub const TICKET_COLORS: &[&str] = &[
    "slate", "bronze", "green", "amber", "red", "blue", "purple", "teal", "pink", "orange", "lime",
    "cyan", "indigo", "magenta", "olive", "brown",
];
