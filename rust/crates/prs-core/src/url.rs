use prs_model::{PullRequestReference, ReviewProvider};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PullRequestUrlError {
    #[error("Enter an absolute GitHub or Azure DevOps pull request URL.")]
    InvalidAbsoluteUrl,
    #[error("The URL is not a supported GitHub or Azure DevOps pull request URL.")]
    Unsupported,
}

pub fn parse(value: &str) -> Result<PullRequestReference, PullRequestUrlError> {
    let uri = Url::parse(value.trim()).map_err(|_| PullRequestUrlError::InvalidAbsoluteUrl)?;
    if !matches!(uri.scheme(), "http" | "https") {
        return Err(PullRequestUrlError::InvalidAbsoluteUrl);
    }

    let segments: Vec<String> = uri
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(percent_decode)
        .collect();

    if let Some(reference) = parse_azure_devops(&uri, &segments) {
        return Ok(reference);
    }
    if let Some(reference) = parse_github(&uri, &segments) {
        return Ok(reference);
    }

    Err(PullRequestUrlError::Unsupported)
}

pub fn try_parse(value: Option<&str>) -> Option<PullRequestReference> {
    value.and_then(|value| parse(value).ok())
}

fn parse_azure_devops(uri: &Url, segments: &[String]) -> Option<PullRequestReference> {
    let host = uri.host_str()?;
    let azure_host = host.eq_ignore_ascii_case("dev.azure.com")
        || host.to_ascii_lowercase().ends_with(".visualstudio.com");
    let git_index = index_of(segments, "_git")?;
    let pull_index = index_of(segments, "pullrequest")?;
    let number = positive_number(segments, pull_index + 1)?;
    if !azure_host || git_index < 1 || pull_index != git_index + 2 {
        return None;
    }

    let (organization, project, api_base_url) = if host.eq_ignore_ascii_case("dev.azure.com") {
        if git_index < 2 {
            return None;
        }
        let organization = segments.first()?.clone();
        let project = segments[1..git_index].join("/");
        let api_base_url = format!(
            "{}://{}/{}/{}",
            uri.scheme(),
            host,
            percent_encode_path(&organization),
            percent_encode_path(&project)
        );
        (organization, project, api_base_url)
    } else {
        let organization = host
            .strip_suffix(".visualstudio.com")
            .unwrap_or(host)
            .to_string();
        let project = segments[..git_index].join("/");
        let api_base_url = format!(
            "{}://{}/{}",
            uri.scheme(),
            host,
            percent_encode_path(&project)
        );
        (organization, project, api_base_url)
    };

    let repository = segments.get(git_index + 1)?.clone();
    let web_url = clean_url(uri);
    let session_key = format!("ado:{organization}/{project}/{repository}#{number}").to_lowercase();

    Some(PullRequestReference {
        provider: ReviewProvider::AzureDevOps,
        host: host.to_string(),
        organization,
        project,
        repository: repository.clone(),
        number,
        web_url,
        api_base_url: api_base_url.clone(),
        clone_url: format!("{api_base_url}/_git/{}", percent_encode_path(&repository)),
        session_key,
    })
}

fn parse_github(uri: &Url, segments: &[String]) -> Option<PullRequestReference> {
    let pull_index = index_of(segments, "pull")?;
    if pull_index < 2 {
        return None;
    }
    let number = positive_number(segments, pull_index + 1)?;
    let owner = segments.get(pull_index - 2)?.clone();
    let repository = segments.get(pull_index - 1)?.clone();
    if owner.trim().is_empty() || repository.trim().is_empty() {
        return None;
    }

    let host = uri.host_str()?;
    let authority = display_authority(uri);
    let api_base_url = if host.eq_ignore_ascii_case("github.com") {
        "https://api.github.com".to_string()
    } else {
        format!("{}://{authority}/api/v3", uri.scheme())
    };
    let session_key = format!("github:{authority}/{owner}/{repository}#{number}").to_lowercase();

    Some(PullRequestReference {
        provider: ReviewProvider::GitHub,
        host: authority.clone(),
        organization: owner.clone(),
        project: owner.clone(),
        repository: repository.clone(),
        number,
        web_url: clean_url(uri),
        api_base_url,
        clone_url: format!(
            "{}://{authority}/{}/{}.git",
            uri.scheme(),
            percent_encode_path(&owner),
            percent_encode_path(&repository)
        ),
        session_key,
    })
}

fn index_of(segments: &[String], value: &str) -> Option<usize> {
    segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case(value))
}

fn positive_number(segments: &[String], index: usize) -> Option<u32> {
    let value = segments.get(index)?.parse().ok()?;
    (value > 0).then_some(value)
}

fn display_authority(uri: &Url) -> String {
    match uri.port() {
        Some(port) => format!("{}:{port}", uri.host_str().unwrap_or_default()),
        None => uri.host_str().unwrap_or_default().to_string(),
    }
}

fn clean_url(uri: &Url) -> String {
    let mut clean = uri.clone();
    clean.set_query(None);
    clean.set_fragment(None);
    clean.as_str().trim_end_matches('/').to_string()
}

fn percent_decode(segment: &str) -> String {
    percent_encoding::percent_decode_str(segment)
        .decode_utf8_lossy()
        .into_owned()
}

fn percent_encode_path(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_pull_request() {
        let reference =
            parse("https://github.com/octo-org/code-review-demo/pull/12345/files").unwrap();
        assert_eq!(reference.organization, "octo-org");
        assert_eq!(reference.repository, "code-review-demo");
        assert_eq!(reference.number, 12345);
        assert_eq!(reference.api_base_url, "https://api.github.com");
        assert_eq!(
            reference.clone_url,
            "https://github.com/octo-org/code-review-demo.git"
        );
    }

    #[test]
    fn parses_enterprise_non_default_port() {
        let reference = parse("http://127.0.0.1:8123/owner/repo/pull/1").unwrap();
        assert_eq!(reference.host, "127.0.0.1:8123");
        assert_eq!(reference.api_base_url, "http://127.0.0.1:8123/api/v3");
    }

    #[test]
    fn parses_azure_devops_pull_request() {
        let reference =
            parse("https://dev.azure.com/contoso/Project/_git/repository/pullrequest/98765")
                .unwrap();
        assert_eq!(reference.provider, ReviewProvider::AzureDevOps);
        assert_eq!(reference.organization, "contoso");
        assert_eq!(reference.project, "Project");
        assert_eq!(reference.repository, "repository");
    }

    #[test]
    fn rejects_unsupported_urls() {
        for value in [
            "",
            "github.com/octo-org/repo/pull/1",
            "https://github.com/octo-org/repo/issues/1",
            "https://dev.azure.com/contoso/Project/_git/repository/pullrequest/nope",
        ] {
            assert!(parse(value).is_err(), "{value}");
        }
    }
}
