//! Repository-structure checks for the `gpui-auto-update` workspace.
//!
//! This crate has no runtime code. Its integration tests inspect the
//! workspace through `cargo metadata` / `cargo tree` and fail when a layering
//! or publication rule from `docs/adr/0001-workspace-layout.md` is broken.
