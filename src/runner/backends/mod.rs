pub(crate) mod agy;
pub(crate) mod claude;
pub(crate) mod codex;
// Issue #527 (1/6): transport + generated-schema plumbing for the Codex
// app-server. Not yet wired into dispatch (a later ticket in the series
// does that), so nothing outside its own tests calls it yet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod codex_app_server;
pub(crate) mod opencode;
pub(crate) mod openhands;
pub(crate) mod vibe;

#[cfg(test)]
pub(crate) mod test_util;
