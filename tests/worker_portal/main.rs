//! End-to-end worker protocol tests on a private bus, without real pointer input.
//!
//! Most tests need their own session bus. Run them with:
//! `dbus-run-session -- cargo test --test worker_portal -- --ignored`
mod app;
mod clicks;
mod eis;
mod fake_eis;
mod gui_reply;
mod hotkey;
mod hotkey_presses;
mod monitors;
mod permission;
mod portal;
mod restart;
mod support;
mod toggle;
