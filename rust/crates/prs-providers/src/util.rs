use serde::de::DeserializeOwned;
use serde_json::Value;
use std::fmt::Write;
use url::Url;

use crate::ProviderError;

pub(crate) fn endpoint(
    api_base_url: &str,
    segments: &[&str],
    query: Option<&str>,
) -> Result<String, ProviderError> {
    let mut url = Url::parse(api_base_url).map_err(|error| {
        ProviderError::Message(format!("The provider API base URL is invalid: {error}"))
    })?;
    url.set_query(None);
    url.set_fragment(None);
    if url.cannot_be_a_base() {
        return Err(ProviderError::Message(
            "The provider API base URL cannot contain path segments.".into(),
        ));
    }

    let mut request = url.as_str().trim_end_matches('/').to_string();
    for segment in segments {
        request.push('/');
        request.push_str(&escape_data_string(segment));
    }
    if let Some(query) = query {
        request.push('?');
        request.push_str(query);
    }
    Ok(request)
}

pub(crate) fn set_query(uri: &str, query: String) -> Result<String, ProviderError> {
    let mut url = Url::parse(uri).map_err(|error| {
        ProviderError::Message(format!("The provider request URL is invalid: {error}"))
    })?;
    url.set_query(Some(&query));
    Ok(url.into())
}

pub(crate) fn value_at<'a>(value: &'a Value, properties: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for property in properties {
        if current.is_null() {
            return None;
        }
        current = current.get(*property)?;
    }
    Some(current)
}

pub(crate) fn string_at<'a>(value: &'a Value, properties: &[&str]) -> Option<&'a str> {
    value_at(value, properties)?.as_str()
}

pub(crate) fn string_value(value: &Value, property: &str) -> String {
    string_at(value, &[property])
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn id_at(value: &Value, property: &str) -> Option<String> {
    let value = value.get(property)?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(crate) fn u32_at(value: &Value, property: &str) -> Option<u32> {
    value
        .get(property)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
}

pub(crate) fn usize_at(value: &Value, property: &str) -> Option<usize> {
    value
        .get(property)?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
}

pub(crate) fn bool_at(value: &Value, property: &str) -> Option<bool> {
    value.get(property)?.as_bool()
}

pub(crate) fn date_at<T>(value: &Value, property: &str) -> Option<T>
where
    T: DeserializeOwned,
{
    date_at_path(value, &[property])
}

pub(crate) fn date_at_path<T>(value: &Value, properties: &[&str]) -> Option<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::String(string_at(value, properties)?.to_string())).ok()
}

fn escape_data_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            escaped.push(char::from(byte));
        } else {
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}
