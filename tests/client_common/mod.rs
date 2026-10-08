//! Helpers shared by the `echosrv-client` black-box tests
//! (`tests/client_cli.rs` and `tests/client_signals.rs`).
#![allow(dead_code)]

use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

pub const BIN: &str = env!("CARGO_BIN_EXE_echosrv-client");
pub const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// The client with the color and logging env vars cleared.
pub fn client_command(args: &[&str]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("RUST_LOG")
        .kill_on_drop(true);
    cmd
}

pub fn describe(out: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn json_lines(out: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("non-JSON line {line:?}: {e}"))
        })
        .collect()
}

pub fn send_signal(child: &tokio::process::Child, signal: libc::c_int) {
    let pid = libc::pid_t::try_from(child.id().expect("client already exited")).unwrap();
    // SAFETY: kill(2) on our own child process.
    assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
}
