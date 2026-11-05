use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

#[derive(Debug, Default)]
pub(super) enum InstanceSelector {
    #[default]
    Local,
    Remote(RemoteInstanceSelector),
}

#[derive(Debug)]
pub(super) struct RemoteInstanceSelector(String);

impl RemoteInstanceSelector {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for InstanceSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let selector = String::deserialize(deserializer)?;
        Ok(if selector == "local" {
            Self::Local
        } else {
            Self::Remote(RemoteInstanceSelector(selector))
        })
    }
}

impl InstanceSelector {
    pub(super) fn deserialize_nullable<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        Ok(Option::<Self>::deserialize(deserializer)?.unwrap_or_default())
    }

    pub(super) fn schema() -> Value {
        json!({
            "type": "string",
            "minLength": 1,
            "description": "Omit or use exact \"local\" for local routing. Otherwise use an enrolled peer UUID or unique saved name; remote errors never fall back locally. The name local is reserved for this Phoenix instance and cannot be used as a peer name."
        })
    }

    pub(super) fn nullable_schema() -> Value {
        json!({
            "oneOf": [
                Self::schema(),
                { "type": "null" }
            ],
            "description": "Omit, use null, or use exact \"local\" for local routing. Otherwise use an enrolled peer UUID or unique saved name; remote errors never fall back locally. The name local is reserved for this Phoenix instance and cannot be used as a peer name."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn only_exact_local_selects_local(selector in prop_oneof![Just("local".to_string()), any::<String>()]) {
            let parsed: InstanceSelector = serde_json::from_value(json!(selector)).unwrap();
            match parsed {
                InstanceSelector::Local => prop_assert_eq!(selector, "local"),
                InstanceSelector::Remote(remote) => {
                    prop_assert_ne!(selector.as_str(), "local");
                    prop_assert_eq!(remote.as_str(), selector.as_str());
                }
            }
        }
    }

    #[test]
    fn local_spelling_variants_and_uuids_remain_remote_selectors() {
        for selector in [
            "Local",
            "LOCAL",
            " local",
            "local ",
            "",
            " ",
            "missing-peer",
            "00000000-0000-4000-8000-000000000001",
        ] {
            let parsed: InstanceSelector = serde_json::from_value(json!(selector)).unwrap();
            let InstanceSelector::Remote(remote) = parsed else {
                panic!("unexpected local alias: {selector:?}");
            };
            assert_eq!(remote.as_str(), selector);
        }
    }
}
