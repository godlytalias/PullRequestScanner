use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::watch;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, PartialEq, Eq)]
pub struct ProcessResult {
    pub exit_code: i32,
    pub standard_output: String,
    pub standard_error: String,
}

impl ProcessResult {
    #[must_use]
    pub fn new(
        exit_code: i32,
        standard_output: impl Into<String>,
        standard_error: impl Into<String>,
    ) -> Self {
        Self {
            exit_code,
            standard_output: standard_output.into(),
            standard_error: standard_error.into(),
        }
    }

    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0
    }
}

impl fmt::Debug for ProcessResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessResult")
            .field("exit_code", &self.exit_code)
            .field("standard_output_len", &self.standard_output.len())
            .field("standard_error_len", &self.standard_error.len())
            .finish_non_exhaustive()
    }
}

impl From<Output> for ProcessResult {
    fn from(output: Output) -> Self {
        Self {
            exit_code: output.status.code().unwrap_or(-1),
            standard_output: String::from_utf8_lossy(&output.stdout).into_owned(),
            standard_error: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to start '{program}': {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed while waiting for '{program}': {source}")]
    Wait {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("process '{program}' timed out after {seconds}s")]
    TimedOut { program: String, seconds: u64 },
    #[error("process '{program}' was cancelled")]
    Cancelled { program: String },
}

impl ProcessError {
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::Spawn { source, .. } => source.kind() == std::io::ErrorKind::NotFound,
            Self::Wait { .. } | Self::TimedOut { .. } | Self::Cancelled { .. } => false,
        }
    }
}

#[derive(Clone)]
pub struct CancellationToken {
    state: watch::Sender<bool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CancellationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CancellationToken")
            .field("is_cancelled", &self.is_cancelled())
            .finish()
    }
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        let (state, _) = watch::channel(false);
        Self { state }
    }

    pub fn cancel(&self) {
        self.state.send_replace(true);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.state.borrow()
    }

    pub async fn cancelled(&self) {
        let mut state = self.state.subscribe();
        loop {
            if *state.borrow_and_update() {
                return;
            }
            if state.changed().await.is_err() {
                return;
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct ProcessOptions {
    pub working_directory: Option<PathBuf>,
    pub environment: HashMap<String, String>,
    pub cancellation_token: Option<CancellationToken>,
}

impl fmt::Debug for ProcessOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut environment_keys = self.environment.keys().collect::<Vec<_>>();
        environment_keys.sort_unstable();
        formatter
            .debug_struct("ProcessOptions")
            .field("working_directory", &self.working_directory)
            .field("environment_keys", &environment_keys)
            .field("cancellation_token", &self.cancellation_token)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct ProcessRunner {
    timeout: Option<Duration>,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessRunner {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
        }
    }

    #[must_use]
    pub const fn without_timeout() -> Self {
        Self { timeout: None }
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    #[must_use]
    pub const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub async fn run<S, I, A>(
        &self,
        file_name: S,
        arguments: I,
    ) -> Result<ProcessResult, ProcessError>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = A>,
        A: AsRef<OsStr>,
    {
        self.run_with_options(file_name, arguments, &ProcessOptions::default())
            .await
    }

    pub async fn run_async<S, I, A>(
        &self,
        file_name: S,
        arguments: I,
        working_directory: Option<&std::path::Path>,
        environment: Option<&HashMap<String, String>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ProcessResult, ProcessError>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = A>,
        A: AsRef<OsStr>,
    {
        let mut options = ProcessOptions {
            working_directory: working_directory.map(PathBuf::from),
            ..ProcessOptions::default()
        };
        if let Some(environment) = environment {
            options.environment.clone_from(environment);
        }
        options.cancellation_token = cancellation_token.cloned();
        self.run_with_options(file_name, arguments, &options).await
    }

    pub async fn run_with_options<S, I, A>(
        &self,
        file_name: S,
        arguments: I,
        options: &ProcessOptions,
    ) -> Result<ProcessResult, ProcessError>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = A>,
        A: AsRef<OsStr>,
    {
        let file_name = OsString::from(file_name.as_ref());
        let display_name = file_name.to_string_lossy().into_owned();
        if options
            .cancellation_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ProcessError::Cancelled {
                program: display_name,
            });
        }
        let mut command = Command::new(&file_name);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(directory) = &options.working_directory {
            command.current_dir(directory);
        }
        for (key, value) in &options.environment {
            command.env(key, value);
        }

        let child = command.spawn().map_err(|source| ProcessError::Spawn {
            program: display_name.clone(),
            source,
        })?;
        let output = match (self.timeout, options.cancellation_token.as_ref()) {
            (Some(timeout), Some(cancellation)) => {
                tokio::select! {
                    result = tokio::time::timeout(timeout, child.wait_with_output()) => {
                        result.map_err(|_| ProcessError::TimedOut {
                            program: display_name.clone(),
                            seconds: timeout.as_secs(),
                        })?
                    }
                    _ = cancellation.cancelled() => {
                        return Err(ProcessError::Cancelled { program: display_name });
                    }
                }
            }
            (Some(timeout), None) => tokio::time::timeout(timeout, child.wait_with_output())
                .await
                .map_err(|_| ProcessError::TimedOut {
                    program: display_name.clone(),
                    seconds: timeout.as_secs(),
                })?,
            (None, Some(cancellation)) => {
                tokio::select! {
                    result = child.wait_with_output() => result,
                    _ = cancellation.cancelled() => {
                        return Err(ProcessError::Cancelled { program: display_name });
                    }
                }
            }
            (None, None) => child.wait_with_output().await,
        };

        Ok(ProcessResult::from(output.map_err(|source| {
            ProcessError::Wait {
                program: display_name,
                source,
            }
        })?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_wait_helper() {
        if let Ok(milliseconds) = std::env::var("PRS_CORE_TEST_SLEEP_MS")
            && let Ok(milliseconds) = milliseconds.parse::<u64>()
        {
            std::thread::sleep(Duration::from_millis(milliseconds));
        }
        if let Ok(value) = std::env::var("PRS_CORE_TEST_CHILD_OUTPUT") {
            println!("{value}");
        }
    }

    #[tokio::test]
    async fn process_result_reports_success_without_dumping_output_in_debug() {
        let result = ProcessResult {
            exit_code: 0,
            standard_output: "secret-output".to_string(),
            standard_error: String::new(),
        };
        let debug = format!("{result:?}");

        assert!(result.succeeded());
        assert!(!debug.contains("secret-output"));
    }

    #[tokio::test]
    async fn cancellation_stops_a_waiting_process() {
        let runner = ProcessRunner::without_timeout();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = runner
            .run_async(
                "definitely-not-a-real-process",
                Vec::<String>::new(),
                None,
                None,
                Some(&cancellation),
            )
            .await
            .expect_err("cancelled process should fail");
        assert!(matches!(error, ProcessError::Cancelled { .. }));
    }

    #[tokio::test]
    async fn timeout_terminates_a_running_process() {
        let executable = std::env::current_exe().unwrap();
        let environment =
            HashMap::from([("PRS_CORE_TEST_SLEEP_MS".to_string(), "10000".to_string())]);
        let error = ProcessRunner::without_timeout()
            .with_timeout(Duration::from_millis(50))
            .run_async(
                executable,
                [
                    "--exact",
                    "process::tests::process_wait_helper",
                    "--nocapture",
                ],
                None,
                Some(&environment),
                None,
            )
            .await
            .expect_err("timed-out process should fail");
        assert!(matches!(error, ProcessError::TimedOut { .. }));
    }

    #[tokio::test]
    async fn cancellation_terminates_a_running_process() {
        let executable = std::env::current_exe().unwrap();
        let environment =
            HashMap::from([("PRS_CORE_TEST_SLEEP_MS".to_string(), "10000".to_string())]);
        let cancellation = CancellationToken::new();
        let child_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            ProcessRunner::without_timeout()
                .run_async(
                    executable,
                    [
                        "--exact",
                        "process::tests::process_wait_helper",
                        "--nocapture",
                    ],
                    None,
                    Some(&environment),
                    Some(&child_cancellation),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancellation.cancel();
        let error = task
            .await
            .unwrap()
            .expect_err("cancelled process should fail");
        assert!(matches!(error, ProcessError::Cancelled { .. }));
    }

    #[tokio::test]
    async fn process_environment_is_passed_without_using_arguments() {
        let executable = std::env::current_exe().unwrap();
        let environment = HashMap::from([(
            "PRS_CORE_TEST_CHILD_OUTPUT".to_string(),
            "environment-value".to_string(),
        )]);
        let result = ProcessRunner::new()
            .run_async(
                executable,
                [
                    "--exact",
                    "process::tests::process_wait_helper",
                    "--nocapture",
                ],
                None,
                Some(&environment),
                None,
            )
            .await
            .unwrap();
        assert!(result.succeeded());
        assert!(result.standard_output.contains("environment-value"));
    }

    #[tokio::test]
    async fn options_debug_redacts_environment_values() {
        let options = ProcessOptions {
            environment: HashMap::from([("TOKEN".to_string(), "top-secret".to_string())]),
            ..ProcessOptions::default()
        };
        let debug = format!("{options:?}");

        assert!(debug.contains("TOKEN"));
        assert!(!debug.contains("top-secret"));
    }
}
