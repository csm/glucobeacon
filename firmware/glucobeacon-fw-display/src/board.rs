//! Heltec WiFi LoRa 32 V3 board facts.
//!
//! Everything here is board-specific and belongs in exactly one place, because
//! the failure mode for a wrong pin is a radio that initializes cleanly and
//! then never receives anything.
//!
//! **These numbers are from the V3 reference design, not measured against a
//! board.** Check them against the schematic for your revision before the first
//! flash.

use glucobeacon_proto::radio::{RadioConfig, Region};

/// Which band to transmit in.
///
/// The SX1262 module on this board covers 863–928 MHz, so the hardware will
/// happily transmit outside the band that is legal where you are. This is the
/// one constant worth checking before the first flash.
pub const REGION: Region = Region::Eu868;

/// The radio settings both ends must share.
pub const RADIO: RadioConfig = RadioConfig::preset(REGION);

/// SX1262 pins, on the board's second SPI bus.
pub mod lora {
    /// SPI clock.
    pub const SCK: u8 = 9;
    /// SPI controller out, peripheral in.
    pub const MOSI: u8 = 10;
    /// SPI controller in, peripheral out.
    pub const MISO: u8 = 11;
    /// Chip select.
    pub const NSS: u8 = 8;
    /// Radio reset, active low.
    pub const RESET: u8 = 12;
    /// Busy line. The SX1262 holds this high while it is working; every
    /// command has to wait for it to fall.
    pub const BUSY: u8 = 13;
    /// Interrupt line, used for transmit-done and receive-done.
    pub const DIO1: u8 = 14;
}

/// The on-board 0.96" SSD1306 OLED, which shares an I2C bus with nothing else.
///
/// Not the product display — the 7-inch e-ink is — but useful during bring-up
/// for showing what the node thinks is going on before the e-ink is wired.
pub mod oled {
    /// I2C data.
    pub const SDA: u8 = 17;
    /// I2C clock.
    pub const SCL: u8 = 18;
    /// Display reset.
    pub const RESET: u8 = 21;
    /// Width in pixels.
    pub const WIDTH: u32 = 128;
    /// Height in pixels.
    pub const HEIGHT: u32 = 64;
}

/// Controls power to the OLED and the header's Vext rail. Active low.
pub const VEXT_CONTROL: u8 = 36;

/// The white LED on the board.
pub const BOARD_LED: u8 = 35;

/// The PRG button, which is also the bootloader strap. Active low.
///
/// Usable as the silence button during bring-up, before the real button is
/// wired — but not in the shipped product, because holding it at reset drops
/// the chip into the bootloader.
pub const PRG_BUTTON: u8 = 0;

/// Battery voltage divider input.
///
/// The board also has a GPIO that must be pulled low to enable the divider,
/// so reading this without asserting [`VBAT_ENABLE`] gives nothing useful.
pub const VBAT_ADC: u8 = 1;

/// Enables the battery voltage divider. Active low.
pub const VBAT_ENABLE: u8 = 37;

/// Pins for the 7-inch e-ink panel, over the board's other SPI bus.
///
/// Chosen from the free header pins rather than fixed by the board — change
/// these to match how you actually wire the panel.
pub mod epaper {
    /// SPI clock.
    pub const SCK: u8 = 3;
    /// SPI data in to the panel.
    pub const MOSI: u8 = 4;
    /// Chip select.
    pub const CS: u8 = 5;
    /// Data/command select.
    pub const DC: u8 = 6;
    /// Panel reset, active low.
    pub const RESET: u8 = 7;
    /// Panel busy line, high while a refresh is in progress.
    pub const BUSY: u8 = 15;
}

/// The piezo buzzer, driven from an LEDC PWM channel.
pub const BUZZER: u8 = 45;

/// The LED inside the silence button.
pub const BUTTON_LED: u8 = 46;

/// The silence button, wired to ground with an internal pull-up. Active low.
pub const SILENCE_BUTTON: u8 = 47;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_radio_preset_is_legal_for_its_region() {
        assert_eq!(RADIO.validate(), Ok(()));
    }

    #[test]
    fn a_reading_every_five_minutes_fits_the_duty_cycle() {
        // A reading frame is about 25 bytes; check the whole protocol maximum
        // so a status or policy packet cannot quietly push us over.
        const FIVE_MINUTES_US: u64 = 300 * 1_000_000;
        assert!(RADIO.fits_duty_cycle(glucobeacon_proto::MAX_FRAME_LEN, FIVE_MINUTES_US));
    }

    #[test]
    fn no_two_peripherals_share_a_pin() {
        // The one mistake in this file that a compiler cannot catch.
        let pins = [
            ("lora.sck", lora::SCK),
            ("lora.mosi", lora::MOSI),
            ("lora.miso", lora::MISO),
            ("lora.nss", lora::NSS),
            ("lora.reset", lora::RESET),
            ("lora.busy", lora::BUSY),
            ("lora.dio1", lora::DIO1),
            ("oled.sda", oled::SDA),
            ("oled.scl", oled::SCL),
            ("oled.reset", oled::RESET),
            ("vext", VEXT_CONTROL),
            ("board_led", BOARD_LED),
            ("prg", PRG_BUTTON),
            ("vbat_adc", VBAT_ADC),
            ("vbat_enable", VBAT_ENABLE),
            ("epaper.sck", epaper::SCK),
            ("epaper.mosi", epaper::MOSI),
            ("epaper.cs", epaper::CS),
            ("epaper.dc", epaper::DC),
            ("epaper.reset", epaper::RESET),
            ("epaper.busy", epaper::BUSY),
            ("buzzer", BUZZER),
            ("button_led", BUTTON_LED),
            ("silence_button", SILENCE_BUTTON),
        ];

        for (i, (name_a, pin_a)) in pins.iter().enumerate() {
            for (name_b, pin_b) in &pins[i + 1..] {
                assert_ne!(pin_a, pin_b, "{name_a} and {name_b} are both GPIO{pin_a}");
            }
        }
    }

    #[test]
    fn every_pin_exists_on_an_esp32_s3() {
        // The S3 has GPIO0-48, with 22-25 absent and 26-32 taken by SPI flash
        // and PSRAM on this module.
        let reserved = 26..=32;
        for pin in [
            lora::SCK,
            lora::MOSI,
            lora::MISO,
            lora::NSS,
            lora::RESET,
            lora::BUSY,
            lora::DIO1,
            epaper::SCK,
            epaper::MOSI,
            epaper::CS,
            epaper::DC,
            epaper::RESET,
            epaper::BUSY,
            BUZZER,
            BUTTON_LED,
            SILENCE_BUTTON,
        ] {
            assert!(pin <= 48, "GPIO{pin} does not exist");
            assert!(!(22..=25).contains(&pin), "GPIO{pin} is absent on the S3");
            assert!(
                !reserved.contains(&pin),
                "GPIO{pin} is used by flash or PSRAM"
            );
        }
    }
}
