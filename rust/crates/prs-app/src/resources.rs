use std::fmt;

const EN_US: &str = include_str!("../resources/en-US/messages.txt");

#[derive(Debug, Clone, Copy, Default)]
pub struct Resources;

impl Resources {
    #[must_use]
    pub const fn en_us() -> Self {
        Self
    }

    pub fn text(self, key: &str) -> Result<&'static str, MissingResource> {
        EN_US
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .find_map(|(candidate, value)| (candidate.trim() == key).then_some(value.trim()))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| MissingResource(key.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingResource(String);

impl fmt::Display for MissingResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "missing en-US resource '{}'", self.0)
    }
}

impl std::error::Error for MissingResource {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_shell_resources() {
        let resources = Resources::en_us();
        assert_eq!(resources.text("App.Title").unwrap(), "Pull Request Scanner");
        assert_eq!(resources.text("OpenReview.Open").unwrap(), "Open review");
    }

    #[test]
    fn missing_resource_is_explicit() {
        assert_eq!(
            Resources::en_us().text("Not.Present").unwrap_err().0,
            "Not.Present"
        );
    }
}
