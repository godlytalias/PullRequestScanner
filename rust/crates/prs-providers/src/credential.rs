use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use prs_core::{CancellationToken, CoreError, CredentialProvider, ReviewCredential};
use prs_model::{PullRequestReference, ReviewProvider as ProviderKind};
use tokio::process::Command;

const AZURE_DEVOPS_RESOURCE: &str = "499b84ac-1321-427f-aa17-267ca6975798";
const GITHUB_ENVIRONMENT_OVERRIDES: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "PULLREQUEST_SCANNER_GITHUB_TOKEN",
];
const AZURE_DEVOPS_ENVIRONMENT_OVERRIDES: &[&str] = &[
    "AZURE_DEVOPS_EXT_PAT",
    "PULLREQUEST_SCANNER_AZURE_DEVOPS_PAT",
    "SYSTEM_ACCESSTOKEN",
];

pub struct CliCredentialProvider {
    ignore_github_environment: AtomicBool,
    ignore_azure_devops_environment: AtomicBool,
}

impl Default for CliCredentialProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CliCredentialProvider {
    #[must_use]
    pub fn new() -> Self {
        Self {
            ignore_github_environment: AtomicBool::new(false),
            ignore_azure_devops_environment: AtomicBool::new(false),
        }
    }

    pub async fn authenticate(&self, reference: &PullRequestReference) -> Result<(), CoreError> {
        self.authenticate_with_cancellation(reference, None).await
    }

    pub async fn authenticate_with_cancellation(
        &self,
        reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<(), CoreError> {
        ensure_not_cancelled(cancellation_token)?;
        let (program, arguments, environment_names) = match reference.provider {
            ProviderKind::GitHub => (
                "gh",
                vec![
                    "auth",
                    "login",
                    "--hostname",
                    reference.host.as_str(),
                    "--git-protocol",
                    "https",
                    "--web",
                    "--skip-ssh-key",
                ],
                GITHUB_ENVIRONMENT_OVERRIDES,
            ),
            ProviderKind::AzureDevOps => (
                "az",
                vec!["login", "--use-device-code", "--allow-no-subscriptions"],
                AZURE_DEVOPS_ENVIRONMENT_OVERRIDES,
            ),
        };

        let mut command = Command::new(program);
        command.args(arguments).kill_on_drop(true);
        for name in environment_names {
            command.env_remove(name);
        }

        let status = run_interactive_command(&mut command, program, cancellation_token).await?;
        if !status.success() {
            return Err(CoreError::operation(format!(
                "{:?} sign-in did not complete successfully (exit {}).",
                reference.provider,
                status.code().unwrap_or(-1)
            )));
        }

        match reference.provider {
            ProviderKind::GitHub => {
                self.ignore_github_environment
                    .store(true, Ordering::Release);
            }
            ProviderKind::AzureDevOps => {
                self.ignore_azure_devops_environment
                    .store(true, Ordering::Release);
            }
        }

        if self
            .get_credential(reference, cancellation_token)
            .await?
            .is_none()
        {
            return Err(CoreError::operation(format!(
                "{:?} sign-in completed, but no usable credential was found. Close the sign-in window only after authentication succeeds, then retry.",
                reference.provider
            )));
        }

        Ok(())
    }

    async fn get_github_credential(
        &self,
        reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        ensure_not_cancelled(cancellation_token)?;
        let environment_token = if self.ignore_github_environment.load(Ordering::Acquire) {
            None
        } else {
            first_environment_value(GITHUB_ENVIRONMENT_OVERRIDES)
        };
        if let Some(token) = usable_trimmed(environment_token) {
            return Ok(Some(ReviewCredential::new("Bearer", token)));
        }

        let token = run_token_command(
            "gh",
            &["auth", "token", "--hostname", reference.host.as_str()],
            cancellation_token,
        )
        .await?;
        Ok(token.map(|value| ReviewCredential::new("Bearer", value)))
    }

    async fn get_azure_devops_credential(
        &self,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        ensure_not_cancelled(cancellation_token)?;
        let environment_pat = if self.ignore_azure_devops_environment.load(Ordering::Acquire) {
            None
        } else {
            first_environment_value(AZURE_DEVOPS_ENVIRONMENT_OVERRIDES)
        };
        if let Some(pat) = usable_trimmed(environment_pat) {
            let parameter = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!(":{pat}").as_bytes(),
            );
            return Ok(Some(ReviewCredential::new("Basic", parameter)));
        }

        let token = run_token_command(
            "az",
            &[
                "account",
                "get-access-token",
                "--resource",
                AZURE_DEVOPS_RESOURCE,
                "--query",
                "accessToken",
                "--output",
                "tsv",
                "--only-show-errors",
            ],
            cancellation_token,
        )
        .await?;
        Ok(token.map(|value| ReviewCredential::new("Bearer", value)))
    }
}

#[async_trait]
impl CredentialProvider for CliCredentialProvider {
    async fn get_credential(
        &self,
        reference: &PullRequestReference,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Option<ReviewCredential>, CoreError> {
        match reference.provider {
            ProviderKind::GitHub => {
                self.get_github_credential(reference, cancellation_token)
                    .await
            }
            ProviderKind::AzureDevOps => self.get_azure_devops_credential(cancellation_token).await,
        }
    }
}

fn first_environment_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| std::env::var(name).ok())
}

fn usable_trimmed(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

async fn run_token_command(
    program: &str,
    arguments: &[&str],
    cancellation_token: Option<&CancellationToken>,
) -> Result<Option<String>, CoreError> {
    ensure_not_cancelled(cancellation_token)?;
    let mut command = Command::new(program);
    command.args(arguments).kill_on_drop(true);
    let output = match cancellation_token {
        Some(token) => {
            tokio::select! {
                output = command.output() => output.ok(),
                () = token.cancelled() => return Err(cancelled_error()),
            }
        }
        None => command.output().await.ok(),
    };
    let Some(output) = output else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }

    Ok(String::from_utf8(output.stdout)
        .ok()
        .and_then(|value| usable_trimmed(Some(value))))
}

async fn run_interactive_command(
    command: &mut Command,
    program: &str,
    cancellation_token: Option<&CancellationToken>,
) -> Result<std::process::ExitStatus, CoreError> {
    match cancellation_token {
        Some(token) => {
            tokio::select! {
                status = command.status() => status.map_err(|error| CoreError::operation(format!(
                    "{program} is required for interactive sign-in but could not be started: {error}."
                ))),
                () = token.cancelled() => Err(cancelled_error()),
            }
        }
        None => command.status().await.map_err(|error| {
            CoreError::operation(format!(
                "{program} is required for interactive sign-in but could not be started: {error}."
            ))
        }),
    }
}

fn ensure_not_cancelled(cancellation_token: Option<&CancellationToken>) -> Result<(), CoreError> {
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(cancelled_error());
    }
    Ok(())
}

fn cancelled_error() -> CoreError {
    CoreError::operation("Credential lookup was cancelled.")
}
