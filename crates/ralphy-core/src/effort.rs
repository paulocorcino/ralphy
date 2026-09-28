use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Vendor-neutral reasoning depth requested by the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl AsRef<str> for Effort {
    fn as_ref(&self) -> &str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_ref())
    }
}

impl FromStr for Effort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(format!(
                "invalid effort '{value}'; expected low, medium, high, xhigh, or max"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Effort;

    #[test]
    fn effort_parses_orders_and_round_trips_the_core_lexicon() {
        let spellings = ["low", "medium", "high", "xhigh", "max"];
        let parsed = spellings
            .iter()
            .map(|spelling| spelling.parse::<Effort>().expect("valid core effort"))
            .collect::<Vec<_>>();

        assert!(parsed.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            parsed.iter().map(ToString::to_string).collect::<Vec<_>>(),
            spellings
        );
        assert_eq!(
            parsed.iter().map(AsRef::<str>::as_ref).collect::<Vec<_>>(),
            spellings
        );

        for invalid in ["none", "minimal", "hihg"] {
            assert!(invalid.parse::<Effort>().is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn effort_serializes_as_its_canonical_string() {
        let high: Effort = serde_json::from_str("\"high\"").expect("deserialize effort");
        assert_eq!(high, Effort::High);
        assert_eq!(serde_json::to_string(&high).unwrap(), "\"high\"");
        assert!(serde_json::from_str::<Effort>("\"none\"").is_err());
    }
}
