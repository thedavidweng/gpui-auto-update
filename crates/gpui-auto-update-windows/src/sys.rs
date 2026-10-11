//! Windows process creation for installer handoff.

use std::io;
use std::os::windows::process::CommandExt as _;
use std::process::{Child, Command, Stdio};

use crate::launch::LaunchedInstaller;
use crate::strategy::InstallerCommand;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const ERROR_ACCESS_DENIED: i32 = 5;

struct Process(Child);

impl LaunchedInstaller for Process {
    fn try_wait(&mut self) -> io::Result<Option<i32>> {
        Ok(self.0.try_wait()?.map(|status| status.code().unwrap_or(-1)))
    }
}

pub(crate) fn launch(command: &InstallerCommand) -> io::Result<Box<dyn LaunchedInstaller>> {
    let base = CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS;
    // Leaving the application's job object keeps a terminal or IDE that
    // kills its job on exit from killing the installer with it. Jobs that
    // forbid breakaway reject the flag, so retry inside the job.
    match spawn(command, base | CREATE_BREAKAWAY_FROM_JOB) {
        Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) => spawn(command, base),
        other => other,
    }
}

fn spawn(command: &InstallerCommand, flags: u32) -> io::Result<Box<dyn LaunchedInstaller>> {
    let mut process = Command::new(command.program());
    for arg in command.args() {
        process.raw_arg(arg);
    }
    if let Some(dir) = command.program().parent() {
        process.current_dir(dir);
    }
    process
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(flags);
    Ok(Box::new(Process(process.spawn()?)))
}
