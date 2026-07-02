//! Child-process execution helpers shared by the random-test runner and the
//! cross-check runner. Data-model independent.

use anyhow::Context as _;
use std::{path::Path, time::Duration};

#[derive(Debug)]
pub(crate) enum RunResult {
    Ok(String),
    RuntimeError(i32),
    TimeLimitExceeded,
}

/// Run `artifact` with `input` on stdin, optionally bounded by `timelimit`.
///
/// `stderr` is discarded. A `None` timelimit means run to completion (used for
/// slow brute-force binaries in cross-check).
pub(crate) fn run_with_input(
    artifact: &Path,
    input: &str,
    timelimit: Option<Duration>,
    cwd: &Path,
) -> anyhow::Result<RunResult> {
    run_with_input_cancellable(artifact, input, timelimit, cwd, || {
        crate::interrupt::requested()
    })
}

fn run_with_input_cancellable(
    artifact: &Path,
    input: &str,
    timelimit: Option<Duration>,
    cwd: &Path,
    cancelled: impl Fn() -> bool,
) -> anyhow::Result<RunResult> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    use std::thread;

    if cancelled() {
        return Err(crate::interrupt::Interrupted.into());
    }

    let mut child = Command::new(artifact)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .current_dir(cwd)
        .spawn()
        .with_context(|| format!("failed to spawn {}", artifact.display()))?;

    let input_bytes = input.as_bytes().to_vec();
    let mut stdin = child.stdin.take().expect("stdin piped");
    // Broken-pipe errors on early process exit are expected and ignored.
    thread::spawn(move || {
        let _ = stdin.write_all(&input_bytes);
    });

    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let stdout_handle = thread::spawn(move || -> Vec<u8> {
        use std::io::Read as _;
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });

    let start = std::time::Instant::now();
    let mut interrupted = false;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if cancelled() {
                    interrupted = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                if timelimit.is_some_and(|limit| start.elapsed() > limit) {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(e.into()),
        }
    };

    let stdout_bytes = stdout_handle.join().unwrap_or_default();

    if interrupted {
        return Err(crate::interrupt::Interrupted.into());
    }

    Ok(match status {
        None if timed_out => RunResult::TimeLimitExceeded,
        None => unreachable!("child wait ended without status, timeout, or interrupt"),
        Some(status) if status.success() => {
            RunResult::Ok(String::from_utf8_lossy(&stdout_bytes).into_owned())
        }
        Some(status) => RunResult::RuntimeError(status.code().unwrap_or(-1)),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Instant,
    };

    #[test]
    fn cancellation_kills_child_without_a_timelimit() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("wait.sh");
        fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let cancelled = Arc::new(AtomicBool::new(false));
        let setter = Arc::clone(&cancelled);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            setter.store(true, Ordering::SeqCst);
        });

        let start = Instant::now();
        let err = run_with_input_cancellable(&script, "", None, dir.path(), || {
            cancelled.load(Ordering::SeqCst)
        })
        .unwrap_err();

        assert!(err
            .downcast_ref::<crate::interrupt::Interrupted>()
            .is_some());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
