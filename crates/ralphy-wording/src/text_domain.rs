use serde::{Deserialize, Serialize};

/// The closed set of text domains: each owns the facts it describes, never a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextDomain {
    Run,
    Adapter,
    Forge,
    WorkingTree,
    Settings,
    Usage,
    Daemon,
    Files,
    Desk,
    Fleet,
    Release,
}

impl TextDomain {
    pub const ALL: [Self; 11] = [
        Self::Run,
        Self::Adapter,
        Self::Forge,
        Self::WorkingTree,
        Self::Settings,
        Self::Usage,
        Self::Daemon,
        Self::Files,
        Self::Desk,
        Self::Fleet,
        Self::Release,
    ];

    /// The kebab-case name used in the domain file and its file name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Adapter => "adapter",
            Self::Forge => "forge",
            Self::WorkingTree => "working-tree",
            Self::Settings => "settings",
            Self::Usage => "usage",
            Self::Daemon => "daemon",
            Self::Files => "files",
            Self::Desk => "desk",
            Self::Fleet => "fleet",
            Self::Release => "release",
        }
    }

    /// The text domain with this name, if there is one.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_holds_the_eleven_kebab_names() {
        let names: Vec<&str> = TextDomain::ALL.iter().map(|d| d.as_str()).collect();
        assert_eq!(
            names,
            [
                "run",
                "adapter",
                "forge",
                "working-tree",
                "settings",
                "usage",
                "daemon",
                "files",
                "desk",
                "fleet",
                "release"
            ]
        );
    }

    #[test]
    fn parse_is_the_inverse_of_as_str() {
        for domain in TextDomain::ALL {
            assert_eq!(TextDomain::parse(domain.as_str()), Some(domain));
        }
        assert_eq!(TextDomain::parse("cli"), None);
        assert_eq!(TextDomain::parse("Run"), None);
    }

    #[test]
    fn serde_names_match_as_str() {
        for domain in TextDomain::ALL {
            let json = serde_json::to_string(&domain).expect("a unit variant serializes");
            assert_eq!(json, format!("\"{}\"", domain.as_str()));
        }
    }
}
