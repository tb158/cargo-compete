use once_cell::sync::Lazy;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

static INTERRUPTED: Lazy<Arc<AtomicBool>> = Lazy::new(|| Arc::new(AtomicBool::new(false)));
static USE_DEFAULT_ACTION: Lazy<Arc<AtomicBool>> = Lazy::new(|| Arc::new(AtomicBool::new(true)));
static INSTALL_RESULT: Lazy<Result<(), String>> = Lazy::new(|| {
    let signal = signal_hook::consts::SIGINT;
    signal_hook::flag::register_conditional_default(signal, Arc::clone(&USE_DEFAULT_ACTION))
        .map_err(|err| err.to_string())?;
    signal_hook::flag::register(signal, Arc::clone(&INTERRUPTED)).map_err(|err| err.to_string())?;
    Ok(())
});

#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

pub(crate) struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        USE_DEFAULT_ACTION.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn activate() -> anyhow::Result<Guard> {
    INTERRUPTED.store(false, Ordering::SeqCst);
    USE_DEFAULT_ACTION.store(false, Ordering::SeqCst);
    if let Err(err) = &*INSTALL_RESULT {
        USE_DEFAULT_ACTION.store(true, Ordering::SeqCst);
        anyhow::bail!("failed to install SIGINT handler: {err}");
    }
    Ok(Guard)
}

pub(crate) fn check() -> anyhow::Result<()> {
    if requested() {
        return Err(Interrupted.into());
    }
    Ok(())
}

pub(crate) fn requested() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        env,
        io::{BufRead as _, BufReader, Write as _},
        os::unix::process::ExitStatusExt as _,
        process::{Child, Command, Stdio},
        thread,
        time::Duration,
    };

    const HELPER_MODE: &str = "CARGO_COMPETE_INTERRUPT_HELPER";

    #[test]
    fn signal_helper() {
        let Ok(mode) = env::var(HELPER_MODE) else {
            return;
        };

        match mode.as_str() {
            "active" => {
                let _guard = activate().unwrap();
                println!("READY");
                std::io::stdout().flush().unwrap();
                while !requested() {
                    thread::sleep(Duration::from_millis(5));
                }
                std::process::exit(130);
            }
            "default" => {
                drop(activate().unwrap());
                println!("READY");
                std::io::stdout().flush().unwrap();
                thread::sleep(Duration::from_secs(30));
                panic!("SIGINT did not terminate the helper");
            }
            _ => panic!("unknown helper mode"),
        }
    }

    fn spawn_helper(mode: &str) -> Child {
        let mut child = Command::new(env::current_exe().unwrap())
            .arg("--exact")
            .arg("interrupt::tests::signal_helper")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(HELPER_MODE, mode)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        assert!(lines.any(|line| line.unwrap().contains("READY")));
        child
    }

    fn send_sigint(child: &Child) {
        let status = Command::new("kill")
            .arg("-INT")
            .arg(child.id().to_string())
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn active_handler_allows_exit_130() {
        let mut child = spawn_helper("active");
        send_sigint(&child);
        assert_eq!(child.wait().unwrap().code(), Some(130));
    }

    #[test]
    fn dropped_guard_restores_default_sigint_behavior() {
        let mut child = spawn_helper("default");
        send_sigint(&child);
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(signal_hook::consts::SIGINT));
    }
}
