//! Controller facade (TICKET-077 through TICKET-083, #357): durable action
//! schema (`action`), pure decision policy (`decision`), abandoned-run and
//! stuck-loop recovery (`recovery`), and daemon/dispatch runtime
//! orchestration (`runtime`) each live in their own bounded module. This
//! file only re-exports the caller-facing surface.

mod action;
pub use self::action::NextAction;

mod decision;
pub use self::decision::decide_next_action;
pub(crate) use self::decision::{is_genuine_agent_failure, AUTO_RETRY_CAP};

mod human_required_reason;
pub use self::human_required_reason::HumanRequiredReason;

mod ownership;
mod recovery;

mod runtime;
pub(crate) use self::runtime::execute_action;
#[allow(unused_imports)]
pub(crate) use self::runtime::loop_parallel_argument;
// `main.rs` declares its own `mod controller` tree (see the `[[bin]]` target
// in Cargo.toml) and is the only caller of this path; that's a separate
// compilation from this lib target, so this re-export is invisible to the
// lib's own reachability analysis and would otherwise warn as unused here.
#[allow(unused_imports)]
pub(crate) use self::runtime::run_dispatch_and_record;
pub use self::runtime::{acquire_profile_lock, run_loop, run_once};
