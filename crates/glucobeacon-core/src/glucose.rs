//! Blood glucose concentration, stored in mg/dL.

/// mg/dL per mmol/L, for glucose specifically (molar mass 180.156 g/mol).
pub const MGDL_PER_MMOL: f32 = 18.0182;

/// A glucose concentration.
///
/// Stored as whole mg/dL because that is what the Dexcom transmitter reports
/// and what the wire protocol carries; mmol/L is derived on demand. Values are
/// unsigned — a negative concentration is not a thing — so subtraction between
/// two readings goes through [`Glucose::delta_mgdl`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Glucose {
    mgdl: u16,
}

impl Glucose {
    /// Lowest value a G6/G7 sensor will report; below this it reads "LOW".
    pub const SENSOR_MIN: Glucose = Glucose { mgdl: 40 };
    /// Highest value a G6/G7 sensor will report; above this it reads "HIGH".
    pub const SENSOR_MAX: Glucose = Glucose { mgdl: 400 };

    /// Wraps a raw mg/dL value.
    pub const fn from_mgdl(mgdl: u16) -> Self {
        Self { mgdl }
    }

    /// Converts from mmol/L, rounding to the nearest mg/dL. Negative and
    /// non-finite inputs clamp to zero.
    pub fn from_mmol_l(mmol: f32) -> Self {
        // NaN is checked explicitly: it compares false against everything, so
        // letting it fall through would cast to an arbitrary value.
        if mmol.is_nan() || mmol <= 0.0 {
            return Self { mgdl: 0 };
        }
        let mgdl = mmol * MGDL_PER_MMOL + 0.5;
        if mgdl >= u16::MAX as f32 {
            return Self { mgdl: u16::MAX };
        }
        Self { mgdl: mgdl as u16 }
    }

    /// The value in mg/dL.
    pub const fn mgdl(self) -> u16 {
        self.mgdl
    }

    /// The value in mmol/L.
    pub fn mmol_l(self) -> f32 {
        self.mgdl as f32 / MGDL_PER_MMOL
    }

    /// The value in tenths of a mmol/L, rounded.
    ///
    /// mmol/L is conventionally shown to one decimal place, and this gets there
    /// without float formatting — which `core::fmt` cannot do on a `no_std`
    /// target anyway.
    pub const fn mmol_l_tenths(self) -> u16 {
        // (mgdl / 18.0182) * 10, scaled to integer math and rounded.
        ((self.mgdl as u32 * 100_000 + 90_091) / 180_182) as u16
    }

    /// Signed difference against an earlier reading, saturating at `i16`'s range.
    pub const fn delta_mgdl(self, earlier: Glucose) -> i16 {
        (self.mgdl as i32 - earlier.mgdl as i32) as i16
    }

    /// Whether the sensor pegged at its bottom of range ("LOW" on the receiver).
    pub const fn is_sensor_low(self) -> bool {
        self.mgdl <= Self::SENSOR_MIN.mgdl
    }

    /// Whether the sensor pegged at its top of range ("HIGH" on the receiver).
    pub const fn is_sensor_high(self) -> bool {
        self.mgdl >= Self::SENSOR_MAX.mgdl
    }
}

impl core::fmt::Display for Glucose {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} mg/dL", self.mgdl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mgdl_round_trips() {
        assert_eq!(Glucose::from_mgdl(120).mgdl(), 120);
    }

    #[test]
    fn mmol_conversion_is_symmetric() {
        for mgdl in [40u16, 70, 100, 180, 250, 400] {
            let g = Glucose::from_mgdl(mgdl);
            let back = Glucose::from_mmol_l(g.mmol_l());
            assert!(
                back.mgdl().abs_diff(mgdl) <= 1,
                "{mgdl} -> {} -> {}",
                g.mmol_l(),
                back.mgdl()
            );
        }
    }

    #[test]
    fn mmol_tenths_matches_float_rounding() {
        for mgdl in 1u16..=400 {
            let g = Glucose::from_mgdl(mgdl);
            let expected = (g.mmol_l() * 10.0 + 0.5) as u16;
            assert_eq!(g.mmol_l_tenths(), expected, "mgdl={mgdl}");
        }
    }

    #[test]
    fn from_mmol_rejects_nonsense() {
        assert_eq!(Glucose::from_mmol_l(-1.0).mgdl(), 0);
        assert_eq!(Glucose::from_mmol_l(f32::NAN).mgdl(), 0);
        assert_eq!(Glucose::from_mmol_l(f32::INFINITY).mgdl(), u16::MAX);
    }

    #[test]
    fn delta_is_signed() {
        let a = Glucose::from_mgdl(120);
        let b = Glucose::from_mgdl(95);
        assert_eq!(b.delta_mgdl(a), -25);
        assert_eq!(a.delta_mgdl(b), 25);
    }

    #[test]
    fn sensor_range_flags() {
        assert!(Glucose::from_mgdl(39).is_sensor_low());
        assert!(Glucose::from_mgdl(40).is_sensor_low());
        assert!(!Glucose::from_mgdl(41).is_sensor_low());
        assert!(Glucose::from_mgdl(400).is_sensor_high());
        assert!(!Glucose::from_mgdl(399).is_sensor_high());
    }
}
