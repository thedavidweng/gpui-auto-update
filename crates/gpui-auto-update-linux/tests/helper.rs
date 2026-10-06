//! The update helper, driven through real child processes.
//!
//! The scenarios live in [`runner`]. They are unix-only, but the test target
//! is declared for every platform (and has `harness = false`), so on other
//! targets this binary must still provide a `main`; it exits successfully.

#[cfg(unix)]
#[path = "helper/runner.rs"]
mod runner;

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    runner::main()
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
