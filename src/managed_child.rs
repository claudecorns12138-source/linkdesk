//! Own and reap only the capture subprocess created by this session.
#[cfg(any(target_os = "macos", test))]
use std::sync::Arc;
use std::{process::Child, sync::Mutex};

#[derive(Default)]
struct State {
    child: Option<Child>,
    stopped: bool,
}

#[derive(Default)]
pub struct SourceControl {
    state: Mutex<State>,
}

fn terminate(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl SourceControl {
    pub fn stop(&self) {
        let child = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.stopped = true;
            state.child.take()
        };
        if let Some(child) = child {
            terminate(child);
        }
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn attach(self: &Arc<Self>, child: Child) -> Option<CaptureSession> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped || state.child.is_some() {
            drop(state);
            terminate(child);
            return None;
        }
        state.child = Some(child);
        Some(CaptureSession(self.clone()))
    }
}
impl Drop for SourceControl {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) struct CaptureSession(Arc<SourceControl>);
#[cfg(any(target_os = "macos", test))]
impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.0.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };

    // The executable is this test binary, not a shell or a downloaded program.
    fn fixture() -> (Child, mpsc::Receiver<()>) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "managed_child::tests::child_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("LINKDESK_CHILD_FIXTURE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (ready_tx, ready) = mpsc::channel();
        let (exit_tx, exited) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if line.contains("LINKDESK_CHILD_READY") {
                    let _ = ready_tx.send(());
                }
            }
            let _ = exit_tx.send(());
        });
        if ready.recv_timeout(Duration::from_secs(5)).is_err() {
            terminate(child);
            panic!("capture subprocess fixture did not start");
        }
        (child, exited)
    }
    #[test]
    #[ignore = "bounded subprocess fixture, started only by lifecycle tests"]
    fn child_fixture() {
        if std::env::var_os("LINKDESK_CHILD_FIXTURE").is_none() {
            return;
        }
        println!("LINKDESK_CHILD_READY");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(15));
    }
    #[test]
    fn stopped_session_reaps_a_late_start_instead_of_leaving_an_orphan() {
        let control = Arc::new(SourceControl::default());
        control.stop();
        let (child, exited) = fixture();
        assert!(control.attach(child).is_none());
        exited
            .recv_timeout(Duration::from_secs(3))
            .expect("late helper must be reaped");
    }
    #[test]
    fn capture_scope_drop_reaps_helper_while_host_handle_is_still_alive() {
        let control = Arc::new(SourceControl::default());
        let (child, exited) = fixture();
        {
            let _session = control.attach(child).unwrap();
        }
        exited
            .recv_timeout(Duration::from_secs(3))
            .expect("capture return/unwind must stop its helper");
        control.stop();
    }
    #[test]
    fn repeated_host_stop_wakes_a_reader_and_session_drop_is_harmless() {
        let control = Arc::new(SourceControl::default());
        let (child, exited) = fixture();
        let session = control.attach(child).unwrap();
        control.stop();
        control.stop();
        exited
            .recv_timeout(Duration::from_secs(3))
            .expect("stop must close the helper pipe");
        drop(session);
    }
}
