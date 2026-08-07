//! LoRa radio parameters, and what they cost in airtime.
//!
//! Both ends must agree on every field of [`RadioConfig`] exactly — a mismatch
//! in any one of them is not a weak link, it is no link, and the symptom is
//! silence rather than an error.
//!
//! The airtime arithmetic is here because it decides two things that are easy
//! to get wrong and hard to notice. First, legality: EU868 caps transmitters at
//! a 1% duty cycle per sub-band, so a spreading factor that seems fine can put
//! you over the limit at one packet every five minutes. Second, whether the
//! link works at all: at SF12 a single frame is over a second of airtime, and
//! two nodes that both transmit will collide.
//!
//! Integer arithmetic throughout, in microseconds — this runs on a chip whose
//! FPU is not worth waking up for it.

/// Spreading factor. Higher is slower, longer range, and much more airtime.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum SpreadingFactor {
    /// SF7: fastest, shortest range.
    Sf7 = 7,
    /// SF8.
    Sf8 = 8,
    /// SF9: a good default for a house-sized link.
    Sf9 = 9,
    /// SF10.
    Sf10 = 10,
    /// SF11.
    Sf11 = 11,
    /// SF12: longest range, and over a second of airtime per frame.
    Sf12 = 12,
}

impl SpreadingFactor {
    /// The numeric spreading factor.
    pub const fn value(self) -> u32 {
        self as u32
    }
}

/// Channel bandwidth.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum Bandwidth {
    /// 125 kHz. The usual choice, and the only one legal on some channels.
    Bw125,
    /// 250 kHz.
    Bw250,
    /// 500 kHz.
    Bw500,
}

impl Bandwidth {
    /// The bandwidth in hertz.
    pub const fn hertz(self) -> u32 {
        match self {
            Self::Bw125 => 125_000,
            Self::Bw250 => 250_000,
            Self::Bw500 => 500_000,
        }
    }
}

/// Forward error correction rate, as the denominator of `4/n`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum CodingRate {
    /// 4/5: least redundancy, least airtime.
    Cr45,
    /// 4/6.
    Cr46,
    /// 4/7.
    Cr47,
    /// 4/8: most redundancy, most airtime.
    Cr48,
}

impl CodingRate {
    /// The `n` in `4/n`.
    pub const fn denominator(self) -> u32 {
        match self {
            Self::Cr45 => 5,
            Self::Cr46 => 6,
            Self::Cr47 => 7,
            Self::Cr48 => 8,
        }
    }
}

/// The regulatory region, which fixes the band and the duty cycle.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum Region {
    /// Europe, 863–870 MHz, 1% duty cycle, 14 dBm.
    Eu868,
    /// North America, 902–928 MHz, no duty cycle limit but a 400 ms dwell-time
    /// cap per transmission.
    Us915,
}

impl Region {
    /// The legal frequency range in hertz, inclusive.
    pub const fn band_hz(self) -> (u32, u32) {
        match self {
            Self::Eu868 => (863_000_000, 870_000_000),
            Self::Us915 => (902_000_000, 928_000_000),
        }
    }

    /// A sensible default frequency for a private point-to-point link.
    pub const fn default_frequency_hz(self) -> u32 {
        match self {
            Self::Eu868 => 868_100_000,
            Self::Us915 => 915_000_000,
        }
    }

    /// The duty cycle cap as a percentage, if the region has one.
    pub const fn duty_cycle_percent(self) -> Option<u32> {
        match self {
            Self::Eu868 => Some(1),
            Self::Us915 => None,
        }
    }

    /// The longest single transmission allowed, if the region caps it.
    ///
    /// US915 has no duty cycle but does limit dwell time, which rules out the
    /// slowest spreading factors on 125 kHz.
    pub const fn max_dwell_time_us(self) -> Option<u64> {
        match self {
            Self::Eu868 => None,
            Self::Us915 => Some(400_000),
        }
    }

    /// The maximum transmit power in dBm.
    pub const fn max_tx_power_dbm(self) -> i8 {
        match self {
            Self::Eu868 => 14,
            Self::Us915 => 21,
        }
    }
}

/// Why a [`RadioConfig`] was rejected.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ConfigError {
    /// The frequency is outside the region's band.
    FrequencyOutOfBand {
        /// The configured frequency.
        frequency_hz: u32,
        /// The legal range.
        band_hz: (u32, u32),
    },
    /// The transmit power exceeds what the region allows.
    TxPowerTooHigh {
        /// The configured power.
        dbm: i8,
        /// The legal maximum.
        max_dbm: i8,
    },
    /// A single frame would exceed the region's dwell time limit.
    DwellTimeExceeded {
        /// Airtime of a maximum-size frame.
        airtime_us: u64,
        /// The legal maximum.
        max_us: u64,
    },
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::FrequencyOutOfBand {
                frequency_hz,
                band_hz,
            } => write!(
                f,
                "{frequency_hz} Hz is outside the {}–{} Hz band",
                band_hz.0, band_hz.1
            ),
            Self::TxPowerTooHigh { dbm, max_dbm } => {
                write!(f, "{dbm} dBm exceeds the {max_dbm} dBm limit")
            }
            Self::DwellTimeExceeded { airtime_us, max_us } => write!(
                f,
                "a full frame takes {airtime_us} us, over the {max_us} us dwell limit"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ConfigError {}

/// Everything both radios must agree on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct RadioConfig {
    /// Which regulatory region this link operates under.
    pub region: Region,
    /// Carrier frequency in hertz.
    pub frequency_hz: u32,
    /// Spreading factor.
    pub spreading_factor: SpreadingFactor,
    /// Channel bandwidth.
    pub bandwidth: Bandwidth,
    /// Forward error correction rate.
    pub coding_rate: CodingRate,
    /// Preamble length in symbols.
    pub preamble_symbols: u16,
    /// Transmit power in dBm.
    pub tx_power_dbm: i8,
    /// Sync word. `0x1424` is the SX126x "private network" value; `0x3444` is
    /// the LoRaWAN public one and would make this link visible to gateways it
    /// has no business talking to.
    pub sync_word: u16,
}

/// The SX126x private-network sync word.
pub const SYNC_WORD_PRIVATE: u16 = 0x1424;

impl RadioConfig {
    /// A starting point for `region`.
    ///
    /// SF9 either way — enough processing gain to cross a house and then some,
    /// without SF12's second-plus of airtime per frame.
    ///
    /// 125 kHz either way. It is the narrowest channel and therefore the most
    /// sensitive, which is what buys range.
    ///
    /// In the EU the binding constraint is the 1% duty cycle, which a reading
    /// every five minutes is nowhere near. In the US, FCC 15.247 offers two
    /// routes into the 902–928 MHz band: frequency hopping, which caps dwell
    /// time at 400 ms per transmission and expects hopping across many
    /// channels, or digital modulation, which requires at least 500 kHz of
    /// bandwidth. A fixed-frequency 125 kHz link is neither, so a US
    /// deployment has a decision to make — see
    /// [`check_airtime`](Self::check_airtime) and
    /// [`max_payload_within_dwell`](Self::max_payload_within_dwell), and
    /// [`Bandwidth::Bw500`] if the wide-band route is preferred to hopping.
    ///
    /// At SF9/125 kHz a 25-byte reading is about 206 ms, inside the dwell cap;
    /// SF10 is not, and a maximum-size frame is not at either.
    pub const fn preset(region: Region) -> Self {
        Self {
            region,
            frequency_hz: region.default_frequency_hz(),
            spreading_factor: SpreadingFactor::Sf9,
            bandwidth: Bandwidth::Bw125,
            coding_rate: CodingRate::Cr45,
            preamble_symbols: 8,
            tx_power_dbm: region.max_tx_power_dbm(),
            sync_word: SYNC_WORD_PRIVATE,
        }
    }

    /// Checks the parts of the configuration that do not depend on what is
    /// being sent: the band and the transmit power.
    ///
    /// Dwell time is *not* checked here, because it depends on the payload —
    /// see [`check_airtime`](Self::check_airtime). A 25-byte reading and a
    /// 206-byte maximum frame are an order of magnitude apart in airtime, and
    /// rejecting a config outright over a frame size the link never actually
    /// sends would rule out the narrow bandwidths that give it range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let band = self.region.band_hz();
        if self.frequency_hz < band.0 || self.frequency_hz > band.1 {
            return Err(ConfigError::FrequencyOutOfBand {
                frequency_hz: self.frequency_hz,
                band_hz: band,
            });
        }

        let max_dbm = self.region.max_tx_power_dbm();
        if self.tx_power_dbm > max_dbm {
            return Err(ConfigError::TxPowerTooHigh {
                dbm: self.tx_power_dbm,
                max_dbm,
            });
        }

        Ok(())
    }

    /// Checks that a `payload_len`-byte frame is inside the region's dwell
    /// limit.
    ///
    /// Call this for the largest frame the link actually sends. Under the
    /// hopping route into the US band the cap is 400 ms per transmission, and
    /// exceeding it is not something the radio will tell you about.
    pub const fn check_airtime(&self, payload_len: usize) -> Result<(), ConfigError> {
        match self.region.max_dwell_time_us() {
            Some(max_us) => {
                let airtime_us = self.airtime_us(payload_len);
                if airtime_us > max_us {
                    Err(ConfigError::DwellTimeExceeded { airtime_us, max_us })
                } else {
                    Ok(())
                }
            }
            None => Ok(()),
        }
    }

    /// The largest frame that fits the region's dwell limit, if it has one.
    ///
    /// `None` where the region does not cap dwell time. `Some(0)` means not
    /// even an empty frame fits, which makes the configuration useless.
    pub fn max_payload_within_dwell(&self) -> Option<usize> {
        self.region.max_dwell_time_us()?;
        // Airtime rises monotonically with payload, so walking down from the
        // protocol maximum finds the largest that fits.
        let mut len = crate::frame::MAX_FRAME_LEN;
        loop {
            if self.check_airtime(len).is_ok() {
                return Some(len);
            }
            if len == 0 {
                return Some(0);
            }
            len -= 1;
        }
    }

    /// Symbol duration in microseconds.
    pub const fn symbol_us(&self) -> u64 {
        (1u64 << self.spreading_factor.value()) * 1_000_000 / self.bandwidth.hertz() as u64
    }

    /// Whether the low data rate optimization is required.
    ///
    /// Mandatory once a symbol lasts longer than 16 ms, which is SF11 and SF12
    /// at 125 kHz. Both ends must enable it together.
    pub const fn low_data_rate_optimize(&self) -> bool {
        self.symbol_us() > 16_000
    }

    /// Time on air, in microseconds, for a `payload_len`-byte frame.
    ///
    /// Assumes an explicit header and CRC on, which is what the framing in
    /// [`crate::frame`] expects.
    pub const fn airtime_us(&self, payload_len: usize) -> u64 {
        let symbol_us = self.symbol_us();
        let sf = self.spreading_factor.value() as i64;

        // (preamble + 4.25) symbols, kept in quarters to stay in integers.
        let preamble_us = (self.preamble_symbols as u64 * 4 + 17) * symbol_us / 4;

        let de = if self.low_data_rate_optimize() { 1 } else { 0 };
        // Explicit header, CRC enabled.
        let numerator = 8 * payload_len as i64 - 4 * sf + 28 + 16;
        let denominator = 4 * (sf - 2 * de);

        let payload_symbols = if numerator <= 0 {
            8
        } else {
            // Rounding up by hand: `i64::div_ceil` is not const-stable, and
            // both operands are known positive here.
            let blocks = (numerator + denominator - 1) / denominator;
            8 + blocks as u64 * self.coding_rate.denominator() as u64
        };

        preamble_us + payload_symbols * symbol_us
    }

    /// The shortest legal gap between transmissions of `payload_len` bytes.
    ///
    /// `None` where the region has no duty cycle limit. Under EU868's 1%, a
    /// frame costing 250 ms of airtime means one transmission every 25
    /// seconds — comfortable at one reading every five minutes, and not
    /// comfortable at all if something starts retrying in a loop.
    pub const fn min_transmit_interval_us(&self, payload_len: usize) -> Option<u64> {
        match self.region.duty_cycle_percent() {
            Some(percent) => Some(self.airtime_us(payload_len) * 100 / percent as u64),
            None => None,
        }
    }

    /// Whether sending a frame every `interval_us` stays inside the duty cycle.
    pub const fn fits_duty_cycle(&self, payload_len: usize, interval_us: u64) -> bool {
        match self.min_transmit_interval_us(payload_len) {
            Some(minimum) => interval_us >= minimum,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typical reading frame, from `frame::tests`.
    const READING_FRAME: usize = 25;

    fn config(sf: SpreadingFactor) -> RadioConfig {
        RadioConfig {
            spreading_factor: sf,
            ..RadioConfig::preset(Region::Eu868)
        }
    }

    #[test]
    fn symbol_durations_match_the_datasheet() {
        assert_eq!(config(SpreadingFactor::Sf7).symbol_us(), 1_024);
        assert_eq!(config(SpreadingFactor::Sf9).symbol_us(), 4_096);
        assert_eq!(config(SpreadingFactor::Sf12).symbol_us(), 32_768);
    }

    /// Semtech's own airtime calculator, for SF7/BW125/CR4-5, 8-symbol
    /// preamble, explicit header, CRC on, 20-byte payload: 56.576 ms.
    #[test]
    fn airtime_matches_the_reference_calculation_at_sf7() {
        assert_eq!(config(SpreadingFactor::Sf7).airtime_us(20), 56_576);
    }

    /// Same parameters at SF12, where the low data rate optimization kicks in:
    /// 1318.912 ms.
    #[test]
    fn airtime_matches_the_reference_calculation_at_sf12() {
        assert_eq!(config(SpreadingFactor::Sf12).airtime_us(20), 1_318_912);
    }

    #[test]
    fn the_low_data_rate_optimization_turns_on_exactly_where_it_must() {
        // Mandatory above a 16 ms symbol: SF11 and SF12 at 125 kHz.
        assert!(!config(SpreadingFactor::Sf10).low_data_rate_optimize());
        assert!(config(SpreadingFactor::Sf11).low_data_rate_optimize());
        assert!(config(SpreadingFactor::Sf12).low_data_rate_optimize());

        // At 500 kHz even SF12 stays under it.
        let wide = RadioConfig {
            bandwidth: Bandwidth::Bw500,
            ..config(SpreadingFactor::Sf12)
        };
        assert!(!wide.low_data_rate_optimize());
    }

    #[test]
    fn airtime_grows_with_the_spreading_factor() {
        let times = [
            SpreadingFactor::Sf7,
            SpreadingFactor::Sf8,
            SpreadingFactor::Sf9,
            SpreadingFactor::Sf10,
            SpreadingFactor::Sf11,
            SpreadingFactor::Sf12,
        ]
        .map(|sf| config(sf).airtime_us(READING_FRAME));

        assert!(times.windows(2).all(|w| w[1] > w[0]), "{times:?}");
    }

    #[test]
    fn airtime_grows_with_the_payload() {
        let cfg = config(SpreadingFactor::Sf9);
        assert!(cfg.airtime_us(200) > cfg.airtime_us(25));
    }

    #[test]
    fn a_tiny_payload_does_not_underflow_the_symbol_count() {
        // The reference formula's numerator goes negative for small payloads at
        // high spreading factors; it must clamp rather than wrap.
        let cfg = config(SpreadingFactor::Sf12);
        assert!(cfg.airtime_us(0) > 0);
        assert!(cfg.airtime_us(1) >= cfg.airtime_us(0));
    }

    #[test]
    fn the_preset_carries_a_reading_well_inside_the_eu_duty_cycle() {
        let cfg = RadioConfig::preset(Region::Eu868);
        assert_eq!(cfg.validate(), Ok(()));

        // One reading every five minutes.
        const FIVE_MINUTES_US: u64 = 300 * 1_000_000;
        assert!(cfg.fits_duty_cycle(READING_FRAME, FIVE_MINUTES_US));

        // And with plenty of headroom: under a minute between transmissions.
        let minimum = cfg
            .min_transmit_interval_us(READING_FRAME)
            .expect("eu868 has a duty cycle");
        assert!(minimum < 60 * 1_000_000, "{minimum} us between packets");
    }

    #[test]
    fn sf12_would_blow_the_eu_duty_cycle_at_a_retry_rate() {
        let cfg = config(SpreadingFactor::Sf12);
        let minimum = cfg
            .min_transmit_interval_us(READING_FRAME)
            .expect("eu868 has a duty cycle");
        // Over two minutes between packets: fine at one every five, and not
        // fine at all for anything that retries.
        assert!(minimum > 120 * 1_000_000, "{minimum} us");
        assert!(!cfg.fits_duty_cycle(READING_FRAME, 30 * 1_000_000));
    }

    #[test]
    fn us915_has_no_duty_cycle_but_does_cap_dwell_time() {
        let cfg = RadioConfig::preset(Region::Us915);
        assert_eq!(cfg.validate(), Ok(()));
        assert_eq!(cfg.min_transmit_interval_us(READING_FRAME), None);
        assert!(cfg.fits_duty_cycle(READING_FRAME, 1));
    }

    #[test]
    fn dwell_time_is_a_question_about_a_payload_not_a_config() {
        // A reading fits at SF9/125 kHz; a maximum-size frame does not. A
        // config check that could not tell those apart would rule out the
        // narrow bandwidth the link needs for range.
        let cfg = RadioConfig::preset(Region::Us915);
        assert_eq!(cfg.bandwidth, Bandwidth::Bw125);
        assert_eq!(cfg.validate(), Ok(()));

        assert_eq!(cfg.check_airtime(READING_FRAME), Ok(()));
        assert!(matches!(
            cfg.check_airtime(crate::frame::MAX_FRAME_LEN),
            Err(ConfigError::DwellTimeExceeded { .. })
        ));
    }

    #[test]
    fn sf10_at_125_khz_just_misses_the_dwell_cap() {
        // 412 ms for a reading, against a 400 ms limit. Worth knowing before
        // reaching for a higher spreading factor to buy range.
        let cfg = RadioConfig {
            spreading_factor: SpreadingFactor::Sf10,
            ..RadioConfig::preset(Region::Us915)
        };
        assert!(matches!(
            cfg.check_airtime(READING_FRAME),
            Err(ConfigError::DwellTimeExceeded { .. })
        ));
    }

    #[test]
    fn the_wide_band_route_carries_a_full_frame() {
        // 500 kHz qualifies as digital modulation under 15.247, which needs no
        // hopping — at the cost of about 6 dB of sensitivity.
        let cfg = RadioConfig {
            bandwidth: Bandwidth::Bw500,
            ..RadioConfig::preset(Region::Us915)
        };
        assert_eq!(cfg.check_airtime(crate::frame::MAX_FRAME_LEN), Ok(()));
    }

    #[test]
    fn the_largest_legal_frame_is_reported() {
        let narrow = RadioConfig::preset(Region::Us915);
        let limit = narrow.max_payload_within_dwell().expect("us caps dwell");
        assert!(limit >= READING_FRAME, "a reading must fit, got {limit}");
        assert!(limit < crate::frame::MAX_FRAME_LEN);
        assert_eq!(narrow.check_airtime(limit), Ok(()));
        assert!(narrow.check_airtime(limit + 1).is_err());

        // The EU has no dwell limit at all.
        assert_eq!(
            RadioConfig::preset(Region::Eu868).max_payload_within_dwell(),
            None
        );
    }

    #[test]
    fn the_eu_preset_is_bounded_by_its_duty_cycle_instead() {
        let cfg = RadioConfig::preset(Region::Eu868);
        assert_eq!(cfg.bandwidth, Bandwidth::Bw125);
        assert_eq!(cfg.validate(), Ok(()));
        assert_eq!(cfg.check_airtime(crate::frame::MAX_FRAME_LEN), Ok(()));
    }

    #[test]
    fn a_frequency_outside_the_band_is_rejected() {
        // The Heltec V3 module covers 863-928 MHz, which spans both bands —
        // so nothing in the hardware stops you configuring the wrong one.
        let cfg = RadioConfig {
            frequency_hz: 915_000_000,
            ..RadioConfig::preset(Region::Eu868)
        };
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::FrequencyOutOfBand { .. })
        ));
    }

    #[test]
    fn transmit_power_is_capped_per_region() {
        // The module can do 21 dBm; EU868 allows 14.
        let cfg = RadioConfig {
            tx_power_dbm: 21,
            ..RadioConfig::preset(Region::Eu868)
        };
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::TxPowerTooHigh {
                dbm: 21,
                max_dbm: 14
            })
        );
        assert_eq!(RadioConfig::preset(Region::Eu868).tx_power_dbm, 14);
        assert_eq!(RadioConfig::preset(Region::Us915).tx_power_dbm, 21);
    }

    #[test]
    fn the_preset_uses_the_private_sync_word() {
        // The public one would put this link on the air where LoRaWAN gateways
        // are listening.
        assert_eq!(
            RadioConfig::preset(Region::Eu868).sync_word,
            SYNC_WORD_PRIVATE
        );
        assert_ne!(SYNC_WORD_PRIVATE, 0x3444, "that is the LoRaWAN sync word");
    }

    #[test]
    fn both_presets_default_inside_their_own_band() {
        for region in [Region::Eu868, Region::Us915] {
            let cfg = RadioConfig::preset(region);
            let (low, high) = region.band_hz();
            assert!((low..=high).contains(&cfg.frequency_hz), "{region:?}");
            assert_eq!(cfg.validate(), Ok(()), "{region:?}");
        }
    }
}
