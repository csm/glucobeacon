//! Rate-of-change direction, as reported by the Dexcom transmitter.

/// The trend arrow shown next to a reading.
///
/// Discriminants match the numeric `Trend` field of the Dexcom Share API, so
/// the wire representation is one byte and needs no lookup table.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum Trend {
    /// No trend reported yet (warm-up, or fewer than three readings).
    #[default]
    None = 0,
    /// Rising faster than 3 mg/dL/min.
    DoubleUp = 1,
    /// Rising 2–3 mg/dL/min.
    SingleUp = 2,
    /// Rising 1–2 mg/dL/min.
    FortyFiveUp = 3,
    /// Steady, within ±1 mg/dL/min.
    Flat = 4,
    /// Falling 1–2 mg/dL/min.
    FortyFiveDown = 5,
    /// Falling 2–3 mg/dL/min.
    SingleDown = 6,
    /// Falling faster than 3 mg/dL/min.
    DoubleDown = 7,
    /// The transmitter could not compute a trend.
    NotComputable = 8,
    /// The rate of change is outside the range the transmitter will report.
    RateOutOfRange = 9,
}

impl Trend {
    /// Parses the numeric `Trend` field from the Share API.
    pub const fn from_share_value(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::None,
            1 => Self::DoubleUp,
            2 => Self::SingleUp,
            3 => Self::FortyFiveUp,
            4 => Self::Flat,
            5 => Self::FortyFiveDown,
            6 => Self::SingleDown,
            7 => Self::DoubleDown,
            8 => Self::NotComputable,
            9 => Self::RateOutOfRange,
            _ => return None,
        })
    }

    /// Parses the string `Direction` field from the Share API, which newer
    /// deployments send in place of (or alongside) the numeric field.
    pub fn from_share_direction(direction: &str) -> Option<Self> {
        Some(match direction {
            "None" => Self::None,
            "DoubleUp" => Self::DoubleUp,
            "SingleUp" => Self::SingleUp,
            "FortyFiveUp" => Self::FortyFiveUp,
            "Flat" => Self::Flat,
            "FortyFiveDown" => Self::FortyFiveDown,
            "SingleDown" => Self::SingleDown,
            "DoubleDown" => Self::DoubleDown,
            "NotComputable" => Self::NotComputable,
            "RateOutOfRange" => Self::RateOutOfRange,
            _ => return None,
        })
    }

    /// The numeric Share API value.
    pub const fn as_share_value(self) -> u8 {
        self as u8
    }

    /// A Unicode arrow for the trend, or `"?"` when there is nothing to draw.
    ///
    /// The e-ink renderer draws its own vector arrows; this is for logs and the
    /// terminal simulator.
    pub const fn arrow(self) -> &'static str {
        match self {
            Self::DoubleUp => "⇈",
            Self::SingleUp => "↑",
            Self::FortyFiveUp => "↗",
            Self::Flat => "→",
            Self::FortyFiveDown => "↘",
            Self::SingleDown => "↓",
            Self::DoubleDown => "⇊",
            Self::None | Self::NotComputable | Self::RateOutOfRange => "?",
        }
    }

    /// Rotation of the trend arrow in degrees, clockwise from "pointing right".
    ///
    /// `None` for the trends that have no direction to point in.
    pub const fn arrow_degrees(self) -> Option<i16> {
        Some(match self {
            Self::DoubleUp | Self::SingleUp => -90,
            Self::FortyFiveUp => -45,
            Self::Flat => 0,
            Self::FortyFiveDown => 45,
            Self::SingleDown | Self::DoubleDown => 90,
            Self::None | Self::NotComputable | Self::RateOutOfRange => return None,
        })
    }

    /// Whether the trend is one of the doubled (fastest) arrows, which matter
    /// for alarm urgency even when the current value is still in range.
    pub const fn is_rapid(self) -> bool {
        matches!(self, Self::DoubleUp | Self::DoubleDown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Trend; 10] = [
        Trend::None,
        Trend::DoubleUp,
        Trend::SingleUp,
        Trend::FortyFiveUp,
        Trend::Flat,
        Trend::FortyFiveDown,
        Trend::SingleDown,
        Trend::DoubleDown,
        Trend::NotComputable,
        Trend::RateOutOfRange,
    ];

    #[test]
    fn share_values_round_trip() {
        for (i, trend) in ALL.into_iter().enumerate() {
            assert_eq!(trend.as_share_value(), i as u8);
            assert_eq!(Trend::from_share_value(i as u8), Some(trend));
        }
        assert_eq!(Trend::from_share_value(10), None);
    }

    #[test]
    fn direction_strings_round_trip() {
        for trend in ALL {
            // The Share `Direction` strings are the variant names verbatim.
            let name = alloc_name(trend);
            assert_eq!(Trend::from_share_direction(name), Some(trend), "{name}");
        }
        assert_eq!(Trend::from_share_direction("Sideways"), None);
    }

    fn alloc_name(trend: Trend) -> &'static str {
        match trend {
            Trend::None => "None",
            Trend::DoubleUp => "DoubleUp",
            Trend::SingleUp => "SingleUp",
            Trend::FortyFiveUp => "FortyFiveUp",
            Trend::Flat => "Flat",
            Trend::FortyFiveDown => "FortyFiveDown",
            Trend::SingleDown => "SingleDown",
            Trend::DoubleDown => "DoubleDown",
            Trend::NotComputable => "NotComputable",
            Trend::RateOutOfRange => "RateOutOfRange",
        }
    }

    #[test]
    fn undirected_trends_have_no_angle() {
        assert_eq!(Trend::Flat.arrow_degrees(), Some(0));
        assert_eq!(Trend::None.arrow_degrees(), None);
        assert_eq!(Trend::NotComputable.arrow_degrees(), None);
        assert_eq!(Trend::RateOutOfRange.arrow_degrees(), None);
    }
}
