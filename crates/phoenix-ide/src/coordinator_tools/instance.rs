use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

#[derive(Debug, Default)]
pub(crate) enum InstanceSelector {
    #[default]
    Local,
    Remote(RemoteInstanceSelector),
}

#[derive(Debug)]
pub(crate) struct RemoteInstanceSelector(String);

impl RemoteInstanceSelector {
    pub(crate) fn as_str(&self) -> &str {
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
    pub(crate) fn deserialize_nullable<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        Ok(Option::<Self>::deserialize(deserializer)?.unwrap_or_default())
    }

    pub(crate) fn schema() -> Value {
        json!({
            "type": "string",
            "minLength": 1,
            "description": "Omit or use exact \"local\" for local routing. Otherwise use an enrolled peer UUID or unique saved name; remote errors never fall back locally. The name local is reserved for this Phoenix instance and cannot be used as a peer name."
        })
    }

    pub(crate) fn nullable_schema() -> Value {
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

pub(crate) enum Route<T> {
    Local,
    Remote(T),
}

#[async_trait::async_trait]
pub(crate) trait RoutedTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> String;
    fn clearable(&self) -> bool;
    fn nullable_instance(&self) -> bool {
        false
    }
    fn schema_without_instance(&self) -> Value;
    fn db(&self) -> &crate::db::Database;
    async fn authorize(&self, _ctx: &ToolContext) -> Result<(), String> {
        Ok(())
    }
    async fn run_routed(
        &self,
        input: Value,
        route: Route<crate::api::federation::ResolvedPeer>,
        ctx: ToolContext,
    ) -> ToolOutput;
}

pub(crate) struct RouteTool<T>(T);

impl<T: RoutedTool + 'static> RouteTool<T> {
    pub(crate) fn new(inner: T) -> Arc<dyn Tool> {
        Arc::new(Self(inner))
    }
}

#[async_trait::async_trait]
impl<T: RoutedTool + 'static> Tool for RouteTool<T> {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn description(&self) -> String {
        self.0.description()
    }
    fn clearable(&self) -> bool {
        self.0.clearable()
    }
    fn input_schema(&self) -> Value {
        let mut schema = self.0.schema_without_instance();
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert(
                "instance".into(),
                if self.0.nullable_instance() {
                    InstanceSelector::nullable_schema()
                } else {
                    InstanceSelector::schema()
                },
            );
        }
        schema
    }
    async fn run(&self, mut input: Value, ctx: ToolContext) -> ToolOutput {
        let selector = match input
            .as_object_mut()
            .and_then(|object| object.remove("instance"))
        {
            None => InstanceSelector::Local,
            Some(Value::Null) if self.0.nullable_instance() => InstanceSelector::Local,
            Some(Value::Null) => {
                return ToolOutput::error(
                    "invalid instance selector: expected a string".to_string(),
                );
            }
            Some(value) => match serde_json::from_value::<InstanceSelector>(value) {
                Ok(selector) => selector,
                Err(error) => {
                    return ToolOutput::error(format!("invalid instance selector: {error}"));
                }
            },
        };
        if let Err(error) = self.0.authorize(&ctx).await {
            return ToolOutput::error(error);
        }
        let route = match selector {
            InstanceSelector::Local => Route::Local,
            InstanceSelector::Remote(selector) => {
                match crate::api::federation::resolve_tool_peer(self.0.db(), &selector).await {
                    Ok(peer) => Route::Remote(peer),
                    Err(error) => return ToolOutput::error(error.to_string()),
                }
            }
        };
        self.0.run_routed(input, route, ctx).await
    }
}

use crate::tools::{Tool, ToolContext, ToolOutput};
use std::sync::Arc;
