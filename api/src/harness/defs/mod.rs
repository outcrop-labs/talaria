// THE DEFS — one module per def family. Each is a prompt, an output contract,
// and what to do when the contract holds or breaks, declared through
// `define.rs` and honored by nobody but `run.rs`.
//
// A def that is not in the registry is invisible in the two ways that matter
// most: the fitness suite cannot replay its fixtures, and the admin panel
// cannot show its floor.

pub mod blurb_writer;
pub mod briefer;
pub mod channel_plan;
pub mod concluder;
pub mod distiller;
pub mod hermes_documents;
pub mod hermes_google;
pub mod hermes_governance;
pub mod hermes_knowledge;
pub mod hermes_research;
pub mod inbox_focus;
pub mod judge;
pub mod librarian;
pub mod muse;
pub mod outreach;
pub mod plan_doc;
pub mod research;
pub mod secret_handles;
pub mod summarizer;
pub mod ticket_relevance;
pub mod titler;
pub mod work_session;
pub mod workbench;
