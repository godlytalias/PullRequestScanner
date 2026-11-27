use prs_core::{CoreError, CredentialProvider};
use prs_model::PullRequestReference;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, USER_AGENT};
use reqwest::{Client, Method, StatusCode};
use serde::Serialize;
use serde_json::Value;

use crate::ProviderError;

#[derive(Clone)]
pub struct ReviewProviderHttp {
    client: Client,
    credentials: std::sync::Arc<dyn CredentialProvider>,
}

impl ReviewProviderHttp {
    pub fn new<C>(client: Client, credentials: C) -> Self
    where
        C: CredentialProvider + 'static,
    {
        Self {
            client,
            credentials: std::sync::Arc::new(credentials),
        }
    }

    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    pub async fn get_json(
        &self,
        reference: &PullRequestReference,
        request_uri: &str,
    ) -> Result<Value, ProviderError> {
        let response = self.send(reference, Method::GET, request_uri, None).await?;
        let body = read_and_validate(reference, response).await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn send_json<T>(
        &self,
        reference: &PullRequestReference,
        method: Method,
        request_uri: &str,
        payload: &T,
    ) -> Result<Value, ProviderError>
    where
        T: Serialize + ?Sized,
    {
        let body = serde_json::to_vec(payload)?;
        let response = self
            .send(reference, method, request_uri, Some(body))
            .await?;
        let body = read_and_validate(reference, response).await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn delete(
        &self,
        reference: &PullRequestReference,
        request_uri: &str,
    ) -> Result<(), ProviderError> {
        let response = self
            .send(reference, Method::DELETE, request_uri, None)
            .await?;
        read_and_validate(reference, response).await?;
        Ok(())
    }

    async fn send(
        &self,
        reference: &PullRequestReference,
        method: Method,
        request_uri: &str,
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut request = self
            .client
            .request(method, request_uri)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "PullRequest-Scanner/1.0");

        if let Some(credential) = self
            .credentials
            .get_credential(reference, None)
            .await
            .map_err(map_credential_error)?
        {
            let authorization = format!("{} {}", credential.scheme, credential.parameter);
            let header = HeaderValue::from_str(&authorization).map_err(|_| {
                ProviderError::Message("The provider credential is not a valid HTTP header.".into())
            })?;
            request = request.header(AUTHORIZATION, header);
        }

        if reference.provider == prs_model::ReviewProvider::GitHub {
            request = request.header("X-GitHub-Api-Version", "2022-11-28");
        }

        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }

        Ok(request.send().await?)
    }
}

async fn read_and_validate(
    reference: &PullRequestReference,
    response: reqwest::Response,
) -> Result<String, ProviderError> {
    let status = response.status();
    let reason = status.canonical_reason().unwrap_or("");
    let body = response.text().await?;
    if status.is_success() {
        return Ok(body);
    }

    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        let authentication_hint = match reference.provider {
            prs_model::ReviewProvider::GitHub => {
                format!(
                    "Run 'gh auth login --hostname {}' or set GH_TOKEN.",
                    reference.host
                )
            }
            prs_model::ReviewProvider::AzureDevOps => {
                "Run 'az login' or set AZURE_DEVOPS_EXT_PAT.".to_string()
            }
        };
        let provider = provider_name(reference.provider);
        return Err(ProviderError::Authentication {
            reference: Box::new(reference.clone()),
            message: format!(
                "{provider} rejected the request ({}). {authentication_hint}",
                status.as_u16()
            ),
        });
    }

    let details: String = body.chars().take(800).collect();
    Err(ProviderError::Message(format!(
        "{} request failed with HTTP {} ({reason}). {details}",
        provider_name(reference.provider),
        status.as_u16()
    )))
}

fn provider_name(provider: prs_model::ReviewProvider) -> &'static str {
    match provider {
        prs_model::ReviewProvider::GitHub => "GitHub",
        prs_model::ReviewProvider::AzureDevOps => "AzureDevOps",
    }
}

fn map_credential_error(error: CoreError) -> ProviderError {
    match error {
        CoreError::Authentication { reference, message } => {
            ProviderError::Authentication { reference, message }
        }
        other => ProviderError::Message(other.to_string()),
    }
}
