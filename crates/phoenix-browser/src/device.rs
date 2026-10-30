//! Session-local CDP device configuration.

use chromiumoxide::cdp::browser_protocol::emulation::{
    SetDeviceMetricsOverrideParams, SetTouchEmulationEnabledParams,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "DeviceInput")]
pub struct DeviceConfig {
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
    touch: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceInput {
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
    touch: bool,
}

impl TryFrom<DeviceInput> for DeviceConfig {
    type Error = String;

    fn try_from(input: DeviceInput) -> Result<Self, Self::Error> {
        if !(1..=16_384).contains(&input.width) || !(1..=16_384).contains(&input.height) {
            return Err("device width and height must be in 1..=16384".into());
        }
        if !input.device_scale_factor.is_finite()
            || input.device_scale_factor <= 0.0
            || input.device_scale_factor > 10.0
        {
            return Err("device_scale_factor must be finite and in (0, 10]".into());
        }
        Ok(Self {
            width: input.width,
            height: input.height,
            device_scale_factor: input.device_scale_factor,
            mobile: input.mobile,
            touch: input.touch,
        })
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            device_scale_factor: 1.0,
            mobile: false,
            touch: false,
        }
    }
}

impl DeviceConfig {
    /// # Errors
    /// Returns an error if touch is disabled or start/move coordinates are not
    /// finite, nonnegative CSS pixels strictly within the configured viewport.
    pub fn validate_touch(self, action: crate::touch::TouchAction) -> Result<(), String> {
        if !self.touch {
            return Err("Touch steps require device.touch=true".into());
        }
        if let crate::touch::TouchAction::Start { x, y }
        | crate::touch::TouchAction::Move { x, y } = action
        {
            if !x.is_finite()
                || !y.is_finite()
                || x < 0.0
                || y < 0.0
                || x >= f64::from(self.width)
                || y >= f64::from(self.height)
            {
                return Err(
                    "Touch coordinates must be finite CSS pixels within the configured viewport"
                        .into(),
                );
            }
        }
        Ok(())
    }

    pub(crate) fn resized(self, width: u32, height: u32) -> Result<Self, String> {
        DeviceInput {
            width,
            height,
            device_scale_factor: self.device_scale_factor,
            mobile: self.mobile,
            touch: self.touch,
        }
        .try_into()
    }

    pub(crate) fn metrics(self) -> SetDeviceMetricsOverrideParams {
        SetDeviceMetricsOverrideParams::new(
            self.width,
            self.height,
            self.device_scale_factor,
            self.mobile,
        )
    }

    pub(crate) fn touch(self) -> SetTouchEmulationEnabledParams {
        SetTouchEmulationEnabledParams::new(self.touch)
    }
}

#[derive(Debug, Default)]
pub(crate) enum DeviceState {
    Known(DeviceConfig),
    #[default]
    Unknown,
}

impl DeviceState {
    pub(crate) async fn configure(
        &mut self,
        page: &chromiumoxide::Page,
        config: DeviceConfig,
    ) -> Result<(), String> {
        self.apply(config, async {
            page.execute(config.touch())
                .await
                .map_err(|e| e.to_string())?;
            page.execute(config.metrics())
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .await
    }

    pub(crate) fn known(&self) -> Result<DeviceConfig, String> {
        match self {
            Self::Known(config) => Ok(*config),
            Self::Unknown => Err(
                "Device configuration is unknown after an incomplete CDP operation; supply an explicit run_scenario device configuration".into(),
            ),
        }
    }

    pub(crate) async fn apply<E>(
        &mut self,
        config: DeviceConfig,
        operation: impl std::future::Future<Output = Result<(), E>>,
    ) -> Result<(), E> {
        *self = Self::Unknown;
        operation.await?;
        *self = Self::Known(config);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mobile() -> DeviceConfig {
        serde_json::from_value(serde_json::json!({
            "width": 390, "height": 844, "device_scale_factor": 3.0,
            "mobile": true, "touch": true
        }))
        .unwrap()
    }

    #[test]
    fn default_metrics_do_not_establish_acknowledged_device() {
        assert!(DeviceState::default().known().is_err());
        let config = DeviceConfig::default();
        let metrics = config.metrics();
        assert_eq!((metrics.width, metrics.height), (1280, 720));
        assert_eq!(metrics.device_scale_factor, 1.0);
        assert!(!metrics.mobile);
        assert!(!config.touch().enabled);
    }

    #[test]
    fn resize_preserves_device_characteristics() {
        let config = mobile().resized(844, 390).unwrap();
        assert_eq!(config.device_scale_factor, 3.0);
        assert!(config.mobile && config.touch);
        assert_eq!(config.metrics().width, 844);
        assert_eq!(config.metrics().height, 390);
        assert!(config.touch().enabled);
        assert!(config.resized(0, 390).is_err());
        assert!(config.resized(390, 16_385).is_err());
    }

    #[test]
    fn rejects_invalid_or_incomplete_configuration() {
        for scale in [0.0, -1.0, 10.1] {
            let mut input = serde_json::to_value(mobile()).unwrap();
            input["device_scale_factor"] = serde_json::json!(scale);
            assert!(serde_json::from_value::<DeviceConfig>(input).is_err());
        }
        assert!(serde_json::from_value::<DeviceConfig>(serde_json::json!({
            "width": 390, "height": 844
        }))
        .is_err());
    }

    #[tokio::test]
    async fn default_configuration_requires_successful_acknowledgement() {
        let mut state = DeviceState::default();
        let config = DeviceConfig::default();
        assert!(state.apply(config, async { Err(()) }).await.is_err());
        assert!(state.known().is_err());
        state
            .apply(config, async { Ok::<_, ()>(()) })
            .await
            .unwrap();
        assert_eq!(state.known().unwrap(), config);
        assert!(!state.known().unwrap().touch().enabled);
    }

    #[tokio::test]
    async fn cancelled_or_failed_apply_is_unknown_until_full_acknowledgement() {
        let mut state = DeviceState::default();
        assert!(state.known().is_err());
        {
            let operation = state.apply(mobile(), std::future::pending::<Result<(), ()>>());
            futures::pin_mut!(operation);
            assert!(futures::poll!(operation).is_pending());
        }
        assert!(state.known().is_err());
        assert!(state.apply(mobile(), async { Err(()) }).await.is_err());
        assert!(state.known().is_err());
        state
            .apply(mobile(), async { Ok::<_, ()>(()) })
            .await
            .unwrap();
        assert_eq!(state.known().unwrap(), mobile());
    }
}
