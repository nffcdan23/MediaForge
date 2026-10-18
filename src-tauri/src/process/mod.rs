//! Launches helper tools and reports their output, with cancellation, timeouts and output limits enforced.
//!
//! There is deliberately no way to pass a shell command line: the executable is an absolute path and the
//! arguments are handed to the operating system as a vector, never interpreted by a shell.

pub mod env;
pub mod lines;
mod tree;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use lines::LineAssembler;
use tree::ProcessTree;

/// How long to wait after asking a tree to stop before forcing it.
const KILL_GRACE: Duration = Duration::from_millis(2500);
/// How long to keep draining pipes after exit in case a grandchild still holds them open.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
const MAX_LINE_LENGTH: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct ProcessRequest {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub working_directory: Option<PathBuf>,
    /// Hard wall-clock limit.
    pub timeout: Option<Duration>,
    /// Stops the process if it stays silent for this long.
    pub idle_timeout: Option<Duration>,
    /// Ceiling on combined stdout and stderr before the process is stopped.
    pub max_output_bytes: usize,
}

impl ProcessRequest {
    pub fn new(executable: impl Into<PathBuf>, args: Vec<OsString>) -> Self {
        Self {
            executable: executable.into(),
            args,
            env: env::sanitized(&[]),
            working_directory: None,
            timeout: None,
            idle_timeout: None,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessExit {
    pub code: Option<i32>,
    /// True when the process was ended by a signal (Unix only).
    pub signalled: bool,
}

impl ProcessExit {
    pub fn success(&self) -> bool {
        !self.signalled && self.code == Some(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    Stdout(String),
    Stderr(String),
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub exit: ProcessExit,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    ExecutableNotFound(String),
    LaunchFailed(String),
    TimedOut(Duration),
    Stalled(Duration),
    OutputLimitExceeded(usize),
    Cancelled,
}

/// Runs a process, calling `on_event` for every line it writes, until it exits.
///
/// Cancelling `cancel` stops the whole process tree and returns `ProcessError::Cancelled`.
pub async fn run_streaming(
    request: &ProcessRequest,
    cancel: &CancellationToken,
    mut on_event: impl FnMut(ProcessEvent),
) -> Result<ProcessExit, ProcessError> {
    let mut stdout_lines = LineAssembler::new(MAX_LINE_LENGTH);
    let mut stderr_lines = LineAssembler::new(MAX_LINE_LENGTH);
    let result = run(request, cancel, |channel, data| match channel {
        Channel::Stdout => stdout_lines.append(data, |line| on_event(ProcessEvent::Stdout(line))),
        Channel::Stderr => stderr_lines.append(data, |line| on_event(ProcessEvent::Stderr(line))),
    })
    .await;
    stdout_lines.flush(|line| on_event(ProcessEvent::Stdout(line)));
    stderr_lines.flush(|line| on_event(ProcessEvent::Stderr(line)));
    result
}

/// Runs a process to completion and returns everything it wrote.
pub async fn run_collect(request: &ProcessRequest, cancel: &CancellationToken) -> Result<ProcessOutput, ProcessError> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = run(request, cancel, |channel, data| match channel {
        Channel::Stdout => stdout.extend_from_slice(data),
        Channel::Stderr => stderr.extend_from_slice(data),
    })
    .await?;
    Ok(ProcessOutput { exit, stdout, stderr: String::from_utf8_lossy(&stderr).into_owned() })
}

/// Runs a process whose stdout is binary (decoded audio, an image), handing over the bytes as they arrive.
/// Diagnostics on stderr are discarded.
pub async fn run_bytes(
    request: &ProcessRequest,
    cancel: &CancellationToken,
    mut on_bytes: impl FnMut(&[u8]),
) -> Result<ProcessExit, ProcessError> {
    run(request, cancel, |channel, data| {
        if let Channel::Stdout = channel {
            on_bytes(data);
        }
    })
    .await
}

#[derive(Debug, Clone, Copy)]
enum Channel {
    Stdout,
    Stderr,
}

fn validate(request: &ProcessRequest) -> Result<(), ProcessError> {
    let path = &request.executable;
    let display = path.display().to_string();
    if !path.is_absolute() || !path.is_file() {
        return Err(ProcessError::ExecutableNotFound(display));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map(|m| m.permissions().mode()).unwrap_or(0);
        if mode & 0o111 == 0 {
            return Err(ProcessError::ExecutableNotFound(display));
        }
    }
    let has_nul = |value: &OsString| {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            value.as_bytes().contains(&0)
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            value.encode_wide().any(|unit| unit == 0)
        }
    };
    if request.args.iter().any(has_nul) {
        return Err(ProcessError::LaunchFailed("An argument contained a NUL character.".into()));
    }
    Ok(())
}

async fn run(
    request: &ProcessRequest,
    cancel: &CancellationToken,
    mut on_chunk: impl FnMut(Channel, &[u8]),
) -> Result<ProcessExit, ProcessError> {
    validate(request)?;
    if cancel.is_cancelled() {
        return Err(ProcessError::Cancelled);
    }

    let mut command = tokio::process::Command::new(&request.executable);
    command
        .args(&request.args)
        .env_clear()
        .envs(request.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    if let Some(directory) = &request.working_directory {
        command.current_dir(directory);
    }
    tree::configure(&mut command);

    let mut child = command.spawn().map_err(|error| ProcessError::LaunchFailed(error.to_string()))?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ProcessError::LaunchFailed(error.to_string()));
        }
    };
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");

    let started = Instant::now();
    let mut last_activity = started;
    let mut failure: Option<ProcessError> = None;
    let mut exit: Option<ProcessExit> = None;
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut output_bytes = 0usize;
    let mut terminated_at: Option<Instant> = None;
    let mut killed = false;
    let mut drain_deadline: Option<Instant> = None;
    let mut stdout_buffer = vec![0u8; 64 * 1024];
    let mut stderr_buffer = vec![0u8; 64 * 1024];

    loop {
        if exit.is_some() && !stdout_open && !stderr_open {
            break;
        }
        let tick = tokio::time::sleep(Duration::from_millis(200));
        tokio::select! {
            biased;
            _ = cancel.cancelled(), if failure.is_none() => {
                failure = Some(ProcessError::Cancelled);
                tree.terminate();
                terminated_at = Some(Instant::now());
            }
            read = stdout.read(&mut stdout_buffer), if stdout_open => match read {
                Ok(0) | Err(_) => stdout_open = false,
                Ok(count) => {
                    last_activity = Instant::now();
                    output_bytes += count;
                    if failure.is_none() {
                        if output_bytes > request.max_output_bytes {
                            failure = Some(ProcessError::OutputLimitExceeded(request.max_output_bytes));
                            tree.terminate();
                            terminated_at = Some(Instant::now());
                        } else {
                            on_chunk(Channel::Stdout, &stdout_buffer[..count]);
                        }
                    }
                }
            },
            read = stderr.read(&mut stderr_buffer), if stderr_open => match read {
                Ok(0) | Err(_) => stderr_open = false,
                Ok(count) => {
                    last_activity = Instant::now();
                    output_bytes += count;
                    if failure.is_none() {
                        if output_bytes > request.max_output_bytes {
                            failure = Some(ProcessError::OutputLimitExceeded(request.max_output_bytes));
                            tree.terminate();
                            terminated_at = Some(Instant::now());
                        } else {
                            on_chunk(Channel::Stderr, &stderr_buffer[..count]);
                        }
                    }
                }
            },
            status = child.wait(), if exit.is_none() => {
                exit = Some(match status {
                    Ok(status) => exit_from(status),
                    Err(_) => ProcessExit { code: None, signalled: false },
                });
                drain_deadline = Some(Instant::now() + DRAIN_GRACE);
            }
            _ = tick => {
                let now = Instant::now();
                if failure.is_none() && exit.is_none() {
                    if let Some(timeout) = request.timeout.filter(|t| now >= started + *t) {
                        failure = Some(ProcessError::TimedOut(timeout));
                    } else if let Some(idle) = request.idle_timeout.filter(|t| now >= last_activity + *t) {
                        failure = Some(ProcessError::Stalled(idle));
                    }
                    if failure.is_some() {
                        tree.terminate();
                        terminated_at = Some(now);
                    }
                }
                if let Some(at) = terminated_at
                    && !killed && now >= at + KILL_GRACE {
                        tree.kill();
                        killed = true;
                    }
                if let Some(deadline) = drain_deadline
                    && now >= deadline && (stdout_open || stderr_open) {
                        // Something in the tree outlived the tool and is holding our pipes. Clean it up and move on.
                        tree.terminate();
                        stdout_open = false;
                        stderr_open = false;
                    }
            }
        }
    }

    // On Windows, closing the job object stops anything the tool left running. Unix process groups need no
    // closing; the group was already signalled above if anything outlived the tool.
    #[allow(clippy::drop_non_drop)]
    drop(tree);
    match (failure, exit) {
        (Some(failure), _) => Err(failure),
        (None, Some(exit)) => Ok(exit),
        (None, None) => Err(ProcessError::LaunchFailed("The process ended unexpectedly.".into())),
    }
}

fn exit_from(status: std::process::ExitStatus) -> ProcessExit {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ProcessExit { code: status.code(), signalled: status.signal().is_some() }
    }
    #[cfg(windows)]
    {
        ProcessExit { code: status.code(), signalled: false }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sh(script: &str) -> ProcessRequest {
        ProcessRequest::new("/bin/sh", vec!["-c".into(), script.into()])
    }

    #[tokio::test]
    async fn captures_stdout_and_the_exit_status() {
        let output = run_collect(&sh("printf hello"), &CancellationToken::new()).await.unwrap();
        assert!(output.exit.success());
        assert_eq!(output.stdout, b"hello");
    }

    #[tokio::test]
    async fn reports_non_zero_exit_codes_and_stderr() {
        let output = run_collect(&sh("echo oops >&2; exit 3"), &CancellationToken::new()).await.unwrap();
        assert_eq!(output.exit.code, Some(3));
        assert!(!output.exit.success());
        assert_eq!(output.stderr.trim(), "oops");
    }

    #[tokio::test]
    async fn streams_lines_in_order_splitting_on_newlines_and_carriage_returns() {
        let mut lines = Vec::new();
        let request = sh("printf 'one\\ntwo\\rthree\\n'; printf 'err\\n' >&2");
        let exit = run_streaming(&request, &CancellationToken::new(), |event| lines.push(event)).await.unwrap();
        assert!(exit.success());
        let stdout: Vec<_> = lines.iter().filter_map(|e| if let ProcessEvent::Stdout(l) = e { Some(l.as_str()) } else { None }).collect();
        assert_eq!(stdout, ["one", "two", "three"]);
        assert!(lines.contains(&ProcessEvent::Stderr("err".into())));
    }

    #[tokio::test]
    async fn arguments_are_never_interpreted_by_a_shell() {
        let hostile = "$(touch /tmp/mediaforge-pwned); `id`; | rm -rf ~";
        let request = ProcessRequest::new("/bin/echo", vec![hostile.into()]);
        let output = run_collect(&request, &CancellationToken::new()).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), hostile);
        assert!(!std::path::Path::new("/tmp/mediaforge-pwned").exists());
    }

    #[tokio::test]
    async fn rejects_a_missing_executable_and_nul_bytes() {
        let missing = ProcessRequest::new("/nonexistent/tool", vec![]);
        assert!(matches!(run_collect(&missing, &CancellationToken::new()).await, Err(ProcessError::ExecutableNotFound(_))));
        let relative = ProcessRequest::new("sh", vec![]);
        assert!(matches!(run_collect(&relative, &CancellationToken::new()).await, Err(ProcessError::ExecutableNotFound(_))));
        let nul = ProcessRequest::new("/bin/echo", vec!["a\0b".into()]);
        assert!(matches!(run_collect(&nul, &CancellationToken::new()).await, Err(ProcessError::LaunchFailed(_))));
    }

    #[tokio::test]
    async fn children_get_the_sanitised_environment() {
        let output = run_collect(&ProcessRequest::new("/usr/bin/env", vec![]), &CancellationToken::new()).await.unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("PATH="));
        assert!(!text.contains("CARGO_"), "the test runner's environment leaked:\n{text}");
    }

    #[tokio::test]
    async fn stops_a_process_that_exceeds_its_time_limit() {
        let mut request = sh("sleep 30");
        request.timeout = Some(Duration::from_millis(500));
        let started = std::time::Instant::now();
        assert_eq!(run_collect(&request, &CancellationToken::new()).await.unwrap_err(), ProcessError::TimedOut(Duration::from_millis(500)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn stops_a_process_that_goes_silent() {
        let mut request = sh("echo started; sleep 30");
        request.idle_timeout = Some(Duration::from_millis(600));
        assert!(matches!(run_collect(&request, &CancellationToken::new()).await, Err(ProcessError::Stalled(_))));
    }

    #[tokio::test]
    async fn stops_a_process_that_produces_too_much_output() {
        let mut request = sh("yes");
        request.max_output_bytes = 100_000;
        assert_eq!(run_collect(&request, &CancellationToken::new()).await.unwrap_err(), ProcessError::OutputLimitExceeded(100_000));
    }

    #[tokio::test]
    async fn cancelling_terminates_the_whole_process_tree() {
        let marker = tempfile::tempdir().unwrap();
        let pid_file = marker.path().join("grandchild.pid");
        // The grandchild ignores nothing and writes its pid; it must not survive cancellation.
        let script = format!("sleep 60 & echo $! > '{}'; wait", pid_file.display());
        let request = sh(&script);
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            trigger.cancel();
        });
        let result = run_collect(&request, &cancel).await;
        assert_eq!(result.unwrap_err(), ProcessError::Cancelled);
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "grandchild {pid} survived cancellation");
    }

    #[tokio::test]
    async fn a_cancelled_token_never_launches() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(run_collect(&sh("echo hi"), &cancel).await.unwrap_err(), ProcessError::Cancelled);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::time::Duration;

    fn cmd(script: &str) -> ProcessRequest {
        let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        ProcessRequest::new(system.join("System32").join("cmd.exe"), vec!["/d".into(), "/c".into(), script.into()])
    }

    fn powershell(script: &str) -> ProcessRequest {
        let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let exe = system.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
        ProcessRequest::new(exe, vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script.into()])
    }

    #[tokio::test]
    async fn captures_stdout_and_exit_codes() {
        let output = run_collect(&cmd("echo hello& exit 3"), &CancellationToken::new()).await.unwrap();
        assert_eq!(output.exit.code, Some(3));
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
    }

    #[tokio::test]
    async fn stops_a_process_that_exceeds_its_time_limit() {
        let mut request = powershell("Start-Sleep -Seconds 30");
        request.timeout = Some(Duration::from_secs(2));
        assert!(matches!(run_collect(&request, &CancellationToken::new()).await, Err(ProcessError::TimedOut(_))));
    }

    #[tokio::test]
    async fn cancelling_terminates_the_whole_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child.pid");
        let script = format!(
            "$p = Start-Process -PassThru -WindowStyle Hidden -FilePath ping -ArgumentList '-n','60','127.0.0.1'; Set-Content -Path '{}' -Value $p.Id; Start-Sleep -Seconds 60",
            pid_file.display()
        );
        let request = powershell(&script);
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            trigger.cancel();
        });
        // Cancel as soon as the grandchild has started.
        let watcher = {
            let pid_file = pid_file.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                for _ in 0..150 {
                    if pid_file.exists() {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        cancel.cancel();
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
        };
        assert_eq!(run_collect(&request, &cancel).await.unwrap_err(), ProcessError::Cancelled);
        watcher.await.unwrap();
        let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let check = run_collect(&cmd(&format!("tasklist /FI \"PID eq {pid}\" /NH")), &CancellationToken::new()).await.unwrap();
        let listing = String::from_utf8_lossy(&check.stdout);
        assert!(!listing.contains(&pid.to_string()), "grandchild {pid} survived: {listing}");
    }
}
