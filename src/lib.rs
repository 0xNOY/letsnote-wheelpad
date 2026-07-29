//! Library surface for letsnote-wheelpad. The binary lives in
//! `src/main.rs`; everything testable lives here so integration tests
//! under `tests/` can reach it.
//!
//! Internal modules are exposed pub(crate) only when they need
//! cross-module access; the OSS surface kept truly public is just
//! enough for synthetic tests to drive the algorithm and FSM directly.

pub mod config;
pub mod detector;
pub mod error;
pub mod evdev;
pub mod fsm;
pub mod runtime;
pub mod uinput;
