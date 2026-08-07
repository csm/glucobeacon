//! The [`Link`] implementation over the SX1262.
//!
//! The protocol already assumes what a LoRa modem gives you — whole packets or
//! nothing, no partial reads, and losses that are normal rather than
//! exceptional — so this is a thin adapter rather than a translation layer.
//!
//! The node spends nearly all its time in receive. A reading arrives every five
//! minutes; between them the radio listens, and `recv_frame` returning
//! `Ok(None)` on timeout is the ordinary case, not an error.

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal::spi::SpiDevice;
use lora_phy::mod_params::{Bandwidth, CodingRate, RadioError, SpreadingFactor};
use lora_phy::sx126x::{Sx1262, Sx126x, TcxoCtrlVoltage};
use lora_phy::{LoRa, RxMode};

use glucobeacon_proto::radio::{self, RadioConfig};
use glucobeacon_proto::{Link, MAX_FRAME_LEN};

/// How long to sit in receive before returning to the caller.
///
/// Short enough that the main loop stays responsive to the button, long enough
/// that it is not thrashing the modem.
const RX_WINDOW_MILLIS: u32 = 20;

/// A [`Link`] over an SX1262.
pub struct LoraLink<SPI, RST, BSY, IRQ, DLY> {
    radio: LoRa<Sx126x<SPI, GenericInterface<RST, BSY, IRQ>, Sx1262>, DLY>,
    config: RadioConfig,
    /// Whether the modem is currently in receive.
    listening: bool,
}

/// Placeholder for `lora_phy`'s interface-variant type, which bundles the
/// reset, busy, and DIO1 pins along with any RF switch control.
///
/// The Heltec V3 has no external RF switch — the SX1262 drives its own — so
/// this is the plain variant with no antenna-select pins.
type GenericInterface<RST, BSY, IRQ> =
    lora_phy::iv::GenericSx126xInterfaceVariant<RST, BSY, IRQ>;

impl<SPI, RST, BSY, IRQ, DLY> LoraLink<SPI, RST, BSY, IRQ, DLY>
where
    SPI: SpiDevice<u8>,
    RST: OutputPin,
    BSY: InputPin,
    IRQ: InputPin,
    DLY: DelayNs,
{
    /// Brings the radio up with `config`.
    ///
    /// Both ends must use the same config exactly. A mismatch in any field is
    /// not a weak link but a silent one, which is why the config is shared
    /// through `glucobeacon-proto` rather than written out at each end.
    pub fn new(
        spi: SPI,
        reset: RST,
        busy: BSY,
        dio1: IRQ,
        config: RadioConfig,
        delay: DLY,
    ) -> Result<Self, RadioError> {
        config
            .validate()
            .expect("the radio config must be legal for its region");

        let interface = GenericInterface::new(reset, busy, dio1, None, None)?;
        // The Heltec V3 fits a TCXO on the SX1262, which needs its supply
        // enabled before the crystal will start.
        let sx_config = lora_phy::sx126x::Config {
            chip: Sx1262,
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: true,
            rx_boost: false,
        };
        let radio = LoRa::new(Sx126x::new(spi, interface, sx_config), false, delay)?;

        Ok(Self {
            radio,
            config,
            listening: false,
        })
    }

    fn modulation(&self) -> Result<lora_phy::mod_params::ModulationParams, RadioError> {
        self.radio.create_modulation_params(
            spreading_factor(self.config.spreading_factor),
            bandwidth(self.config.bandwidth),
            coding_rate(self.config.coding_rate),
            self.config.frequency_hz,
        )
    }

    /// Puts the modem into continuous receive if it is not already.
    fn listen(&mut self) -> Result<(), RadioError> {
        if self.listening {
            return Ok(());
        }
        let modulation = self.modulation()?;
        let mut packet_params = self.radio.create_rx_packet_params(
            self.config.preamble_symbols,
            false, // explicit header, matching what `frame` assumes
            MAX_FRAME_LEN as u8,
            true,  // CRC on
            false, // IQ not inverted
            &modulation,
        )?;
        self.radio
            .prepare_for_rx(RxMode::Continuous, &modulation, &mut packet_params)
            .await_blocking()?;
        self.listening = true;
        Ok(())
    }
}

impl<SPI, RST, BSY, IRQ, DLY> Link for LoraLink<SPI, RST, BSY, IRQ, DLY>
where
    SPI: SpiDevice<u8>,
    RST: OutputPin,
    BSY: InputPin,
    IRQ: InputPin,
    DLY: DelayNs,
{
    type Error = RadioError;

    fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        // Transmitting drops us out of receive; the next recv puts us back.
        self.listening = false;

        let modulation = self.modulation()?;
        let mut packet_params = self.radio.create_tx_packet_params(
            self.config.preamble_symbols,
            false,
            true,
            false,
            &modulation,
        )?;
        self.radio.prepare_for_tx(
            &modulation,
            &mut packet_params,
            self.config.tx_power_dbm.into(),
            frame,
        )?;
        self.radio.tx()?;
        Ok(())
    }

    fn recv_frame(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        self.listen()?;

        match self.radio.rx_with_timeout(RX_WINDOW_MILLIS, buf) {
            Ok((len, _status)) => Ok(Some(len as usize)),
            // A quiet window is the normal case on a link that carries one
            // packet every five minutes.
            Err(RadioError::ReceiveTimeout) => Ok(None),
            // A bad CRC means the frame was mangled in the air. The framing
            // layer would reject it anyway; drop it here and keep listening.
            Err(RadioError::CRCErrorOnReceive) => Ok(None),
            Err(other) => {
                // Anything else may have left the modem in a state we cannot
                // reason about, so re-arm receive from scratch next time.
                self.listening = false;
                Err(other)
            }
        }
    }
}

const fn spreading_factor(sf: radio::SpreadingFactor) -> SpreadingFactor {
    match sf {
        radio::SpreadingFactor::Sf7 => SpreadingFactor::_7,
        radio::SpreadingFactor::Sf8 => SpreadingFactor::_8,
        radio::SpreadingFactor::Sf9 => SpreadingFactor::_9,
        radio::SpreadingFactor::Sf10 => SpreadingFactor::_10,
        radio::SpreadingFactor::Sf11 => SpreadingFactor::_11,
        radio::SpreadingFactor::Sf12 => SpreadingFactor::_12,
    }
}

const fn bandwidth(bw: radio::Bandwidth) -> Bandwidth {
    match bw {
        radio::Bandwidth::Bw125 => Bandwidth::_125KHz,
        radio::Bandwidth::Bw250 => Bandwidth::_250KHz,
        radio::Bandwidth::Bw500 => Bandwidth::_500KHz,
    }
}

const fn coding_rate(cr: radio::CodingRate) -> CodingRate {
    match cr {
        radio::CodingRate::Cr45 => CodingRate::_4_5,
        radio::CodingRate::Cr46 => CodingRate::_4_6,
        radio::CodingRate::Cr47 => CodingRate::_4_7,
        radio::CodingRate::Cr48 => CodingRate::_4_8,
    }
}
