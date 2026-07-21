use serde::{Deserialize, Serialize};

/// How much a woken manager agent (see `Defaults::current_manager`) is
/// allowed to do on its own when a notify-worthy event fires (MR ready,
/// human required, review verdict, terminal dispatch failure).
/// Deliberately per-profile, not global -- an operator sprinting on one
/// project may want full autonomy while another project's operator wants
/// to decide every merge themselves.
///
/// * `Off` (default): no wake, `notify_command` behavior is unchanged.
/// * `ReviewOnly`: the woken agent reviews and posts findings, but must not
///   merge or take any other write action.
/// * `Full`: the woken agent may act on its own judgment (review and merge
///   if CI is green and review passed, investigate and fix a failure,
///   etc.) under the same standing authorization a human operator would
///   otherwise apply manually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WakeAutonomy {
    #[default]
    Off,
    ReviewOnly,
    Full,
}
