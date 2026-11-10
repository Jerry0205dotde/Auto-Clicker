use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Highest rate of single clicks (press/release pairs), for every click type.
pub const MAX_CLICKS_PER_SECOND: u64 = 500;
pub(crate) const MAX_INTERVAL_HOURS: u64 = 24;
pub const MAX_INTERVAL_MS: u64 = MAX_INTERVAL_HOURS * 60 * 60 * 1_000;
pub const MAX_REPEAT_COUNT: u64 = 10_000_000;
/// Largest fixed-position coordinate on either axis.
pub const MAX_COORDINATE: u32 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

/// Index of the button in the UI and its controller property.
impl From<MouseButton> for i32 {
    fn from(button: MouseButton) -> Self {
        match button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        }
    }
}

impl TryFrom<i32> for MouseButton {
    type Error = ValidationError;

    fn try_from(index: i32) -> Result<Self, Self::Error> {
        match index {
            0 => Ok(Self::Left),
            1 => Ok(Self::Right),
            2 => Ok(Self::Middle),
            _ => Err(ValidationError::InvalidMouseButton),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClickType {
    #[default]
    Single,
    Double,
}

impl ClickType {
    pub const fn clicks_per_tick(self) -> u8 {
        match self {
            Self::Single => 1,
            Self::Double => 2,
        }
    }

    /// Shortest interval that keeps this click type at or below the maximum
    /// click rate: 2 ms for single and 4 ms for double clicks.
    pub const fn min_interval_ms(self) -> u64 {
        1_000 * self.clicks_per_tick() as u64 / MAX_CLICKS_PER_SECOND
    }
}

/// Index of the click type in the UI and its controller property.
impl From<ClickType> for i32 {
    fn from(click_type: ClickType) -> Self {
        match click_type {
            ClickType::Single => 0,
            ClickType::Double => 1,
        }
    }
}

impl TryFrom<i32> for ClickType {
    type Error = ValidationError;

    fn try_from(index: i32) -> Result<Self, Self::Error> {
        match index {
            0 => Ok(Self::Single),
            1 => Ok(Self::Double),
            _ => Err(ValidationError::InvalidClickType),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    UntilStopped,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionMode {
    CurrentCursor,
    Fixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickSettings {
    pub interval_ms: u64,
    pub button: MouseButton,
    pub click_type: ClickType,
    pub repeat: Option<u64>,
    pub position: Option<(u32, u32)>,
    pub monitor: Option<MonitorGeometry>,
}

/// Why click settings are invalid. `messages` holds the shown texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    IntervalTooShort { min_interval_ms: u64 },
    IntervalTooLong,
    InvalidRepeat,
    InvalidCoordinates,
    InvalidMonitorPosition,
    InvalidMouseButton,
    InvalidClickType,
}

impl std::error::Error for ValidationError {}

impl ClickSettings {
    /// Validate timing, repeat limits and fixed coordinates against their monitor.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_interval(self.interval_ms, self.click_type)?;
        if let Some(repeat) = self.repeat
            && !(1..=MAX_REPEAT_COUNT).contains(&repeat)
        {
            return Err(ValidationError::InvalidRepeat);
        }
        if let Some((x, y)) = self.position {
            if x > MAX_COORDINATE || y > MAX_COORDINATE {
                return Err(ValidationError::InvalidCoordinates);
            }
            let monitor = self
                .monitor
                .ok_or(ValidationError::InvalidMonitorPosition)?;
            if monitor.width <= 0
                || monitor.height <= 0
                || x >= monitor.width as u32
                || y >= monitor.height as u32
            {
                return Err(ValidationError::InvalidMonitorPosition);
            }
        }
        Ok(())
    }

    pub const fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms)
    }
}

pub fn validate_interval(interval_ms: u64, click_type: ClickType) -> Result<(), ValidationError> {
    let min_interval_ms = click_type.min_interval_ms();
    if interval_ms < min_interval_ms {
        Err(ValidationError::IntervalTooShort { min_interval_ms })
    } else if interval_ms > MAX_INTERVAL_MS {
        Err(ValidationError::IntervalTooLong)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_settings() -> ClickSettings {
        ClickSettings {
            interval_ms: 100,
            button: MouseButton::Left,
            click_type: ClickType::Single,
            repeat: None,
            position: None,
            monitor: None,
        }
    }

    #[test]
    fn validates_interval_and_click_rate_limit() {
        // 500 single clicks/s: 500 single or 250 double cycles per second.
        for (click_type, min_interval_ms) in [(ClickType::Single, 2), (ClickType::Double, 4)] {
            assert_eq!(click_type.min_interval_ms(), min_interval_ms);
            for too_short in [0, min_interval_ms - 1] {
                assert_eq!(
                    validate_interval(too_short, click_type),
                    Err(ValidationError::IntervalTooShort { min_interval_ms })
                );
            }
            for valid in [min_interval_ms, 10, 1_000, MAX_INTERVAL_MS] {
                assert!(validate_interval(valid, click_type).is_ok());
            }
            assert_eq!(
                validate_interval(MAX_INTERVAL_MS + 1, click_type),
                Err(ValidationError::IntervalTooLong)
            );
        }
    }

    #[test]
    fn settings_apply_the_limit_of_their_click_type() {
        let mut settings = valid_settings();
        settings.interval_ms = 2;
        assert!(settings.validate().is_ok());
        settings.click_type = ClickType::Double;
        assert_eq!(
            settings.validate(),
            Err(ValidationError::IntervalTooShort { min_interval_ms: 4 })
        );
        settings.interval_ms = 4;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn validates_repeat_count() {
        let mut settings = valid_settings();
        settings.repeat = Some(0);
        assert_eq!(settings.validate(), Err(ValidationError::InvalidRepeat));
        settings.repeat = Some(100);
        assert!(settings.validate().is_ok());
        settings.repeat = Some(MAX_REPEAT_COUNT + 1);
        assert_eq!(settings.validate(), Err(ValidationError::InvalidRepeat));
    }

    #[test]
    fn fixed_position_requires_monitor_and_stays_inside_edges() {
        let mut settings = valid_settings();
        settings.position = Some((1919, 1079));
        assert_eq!(
            settings.validate(),
            Err(ValidationError::InvalidMonitorPosition)
        );
        settings.monitor = Some(MonitorGeometry {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        });
        assert!(settings.validate().is_ok());
        settings.position = Some((1920, 0));
        assert_eq!(
            settings.validate(),
            Err(ValidationError::InvalidMonitorPosition)
        );
        settings.position = Some((0, 1080));
        assert_eq!(
            settings.validate(),
            Err(ValidationError::InvalidMonitorPosition)
        );
    }

    #[test]
    fn maps_ui_indexes_both_ways() {
        for (index, button) in [
            (0, MouseButton::Left),
            (1, MouseButton::Right),
            (2, MouseButton::Middle),
        ] {
            assert_eq!(MouseButton::try_from(index), Ok(button));
            assert_eq!(i32::from(button), index);
        }
        for (index, click_type) in [(0, ClickType::Single), (1, ClickType::Double)] {
            assert_eq!(ClickType::try_from(index), Ok(click_type));
            assert_eq!(i32::from(click_type), index);
        }
        for index in [-1, 3] {
            assert_eq!(
                MouseButton::try_from(index),
                Err(ValidationError::InvalidMouseButton)
            );
        }
        for index in [-1, 2] {
            assert_eq!(
                ClickType::try_from(index),
                Err(ValidationError::InvalidClickType)
            );
        }
    }
}
