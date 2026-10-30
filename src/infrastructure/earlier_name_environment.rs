// Variables named after the product's earlier name.
//
// Until 2026-10-02 the product was Crosscode and every variable it read began `CROSSCODE_`.
// A variable that is still set under that name and simply ignored would leave a run reading
// another data directory or another policy file than the person thinks, with nothing to say
// so. The rule is one prefix and not a list: a list here would have to grow with every
// variable added later, and the one somebody forgot to add would be ignored silently.
use std::ffi::OsString;

const EARLIER_PREFIX: &str = "CROSSCODE_";
const PREFIX: &str = "CROSSKIT_";

/// One variable set under the earlier name, and the name it has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamedVariable {
    pub earlier: String,
    pub current: String,
}

/// What the environment holds under the earlier name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EarlierNameEnvironment {
    /// Set under the earlier name only: the run must not go on as if it were absent.
    pub unrenamed: Vec<RenamedVariable>,
    /// Set under both names: the current one is read and the earlier one is ignored, aloud.
    pub overridden: Vec<RenamedVariable>,
}

/// The process environment, read once.
pub fn earlier_name_environment() -> EarlierNameEnvironment {
    earlier_name_environment_of(std::env::vars_os())
}

fn earlier_name_environment_of(
    variables: impl Iterator<Item = (OsString, OsString)>,
) -> EarlierNameEnvironment {
    let names: Vec<String> = variables
        .filter_map(|(name, _)| name.into_string().ok())
        .collect();
    let mut found = EarlierNameEnvironment::default();
    let mut earlier: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with(EARLIER_PREFIX))
        .collect();
    earlier.sort();
    for name in earlier {
        let renamed = RenamedVariable {
            earlier: name.clone(),
            current: format!("{PREFIX}{}", &name[EARLIER_PREFIX.len()..]),
        };
        if names.contains(&renamed.current) {
            found.overridden.push(renamed);
        } else {
            found.unrenamed.push(renamed);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{RenamedVariable, earlier_name_environment_of};

    fn environment(names: &[&str]) -> super::EarlierNameEnvironment {
        earlier_name_environment_of(
            names
                .iter()
                .map(|name| (OsString::from(name), OsString::from("value"))),
        )
    }

    fn renamed(earlier: &str, current: &str) -> RenamedVariable {
        RenamedVariable {
            earlier: earlier.to_string(),
            current: current.to_string(),
        }
    }

    #[test]
    fn a_variable_under_the_earlier_name_alone_is_named_with_its_new_name() {
        let found = environment(&["HOME", "CROSSCODE_DATA_DIR"]);
        assert_eq!(
            found.unrenamed,
            vec![renamed("CROSSCODE_DATA_DIR", "CROSSKIT_DATA_DIR")]
        );
        assert!(found.overridden.is_empty());
    }

    #[test]
    fn under_both_names_the_new_one_wins_and_the_earlier_one_is_reported() {
        let found = environment(&["CROSSCODE_CONFIG", "CROSSKIT_CONFIG"]);
        assert!(found.unrenamed.is_empty());
        assert_eq!(
            found.overridden,
            vec![renamed("CROSSCODE_CONFIG", "CROSSKIT_CONFIG")]
        );
    }

    /// The rule is the prefix: a variable this build never heard of is still caught.
    #[test]
    fn any_variable_with_the_earlier_prefix_is_caught_not_only_known_ones() {
        let found = environment(&["CROSSCODE_SOMETHING_LATER", "CROSSKIT_DATA_DIR"]);
        assert_eq!(
            found.unrenamed,
            vec![renamed(
                "CROSSCODE_SOMETHING_LATER",
                "CROSSKIT_SOMETHING_LATER"
            )]
        );
    }

    #[test]
    fn an_environment_with_only_current_names_has_nothing_to_say() {
        assert_eq!(
            environment(&["CROSSKIT_DATA_DIR", "PATH"]),
            Default::default()
        );
    }
}
