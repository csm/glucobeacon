//! Which Share deployment to talk to.

use std::fmt;
use std::str::FromStr;

/// The Dexcom Share server serving a given account.
///
/// Accounts are not portable between these: a European account does not exist
/// on the US server, and logging in against the wrong one looks exactly like a
/// wrong password. Getting this right is the single most common setup mistake.
#[derive(
    Copy, Clone, PartialEq, Eq, Hash, Debug, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    /// United States.
    #[default]
    Us,
    /// Outside the United States, except Japan.
    Ous,
    /// Japan.
    Japan,
}

impl Region {
    /// Every region, for enumerating in help text.
    pub const ALL: [Region; 3] = [Region::Us, Region::Ous, Region::Japan];

    /// The Share host for this region.
    pub const fn base_url(self) -> &'static str {
        match self {
            Self::Us => "https://share2.dexcom.com",
            Self::Ous => "https://shareous1.dexcom.com",
            Self::Japan => "https://sharejp.dexcom.com",
        }
    }

    /// The short name accepted on the command line and in config.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Us => "us",
            Self::Ous => "ous",
            Self::Japan => "jp",
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Region {
    type Err = UnknownRegion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "us" | "usa" | "united-states" => Ok(Self::Us),
            "ous" | "eu" | "non-us" => Ok(Self::Ous),
            "jp" | "japan" => Ok(Self::Japan),
            _ => Err(UnknownRegion(s.to_owned())),
        }
    }
}

/// The region string did not name a Share deployment.
#[derive(Clone, Debug)]
pub struct UnknownRegion(String);

impl fmt::Display for UnknownRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown Dexcom region {:?}; expected one of: ", self.0)?;
        for (i, region) in Region::ALL.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(region.as_str())?;
        }
        Ok(())
    }
}

impl std::error::Error for UnknownRegion {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_round_trip() {
        for region in Region::ALL {
            assert_eq!(region.as_str().parse::<Region>().expect("parse"), region);
        }
    }

    #[test]
    fn parsing_is_forgiving_about_case_and_aliases() {
        assert_eq!("US".parse::<Region>().expect("parse"), Region::Us);
        assert_eq!("  eu ".parse::<Region>().expect("parse"), Region::Ous);
        assert_eq!("Japan".parse::<Region>().expect("parse"), Region::Japan);
    }

    #[test]
    fn an_unknown_region_lists_the_valid_ones() {
        let err = "mars".parse::<Region>().expect_err("should reject");
        let message = err.to_string();
        assert!(message.contains("mars"), "{message}");
        assert!(message.contains("ous"), "{message}");
    }

    #[test]
    fn every_region_has_a_distinct_host() {
        let mut hosts: Vec<&str> = Region::ALL.iter().map(|r| r.base_url()).collect();
        hosts.sort_unstable();
        hosts.dedup();
        assert_eq!(hosts.len(), Region::ALL.len());
        assert!(hosts.iter().all(|h| h.starts_with("https://")));
    }
}
