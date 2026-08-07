# Firmware

Two binaries for the Heltec WiFi LoRa 32 V3 (ESP32-S3FN8 + SX1262).

These crates are **excluded from the workspace**. They need the Xtensa
toolchain and a device target, so building them from the workspace root would
mean nothing else could be built without it.

```
firmware/
  glucobeacon-fw-gateway   ESP-IDF (std): WiFi, Dexcom, SX1262 transmit
  glucobeacon-fw-display   esp-hal (no_std): SX1262 receive, e-ink, buzzer, button
```

## Status

Skeletons, written but **not yet compiled** — see "What is not verified" below.

The logic they depend on is written and tested on the host, and the crates it
comes from are verified to compile for `xtensa-esp32s3-none-elf`:

- `glucobeacon-core` — readings, alarm state machine
- `glucobeacon-proto` — framing, the `Link` trait, and `radio::RadioConfig`
  with the airtime and duty-cycle arithmetic
- `glucobeacon-display` — panel layout, packed framebuffer, buzzer and LED
  pattern timing

What is left in these two crates is the wiring: SPI, GPIO, and the peripheral
traits.

## Toolchain

The ESP32-S3 is Xtensa, so stock rustup will not do. Install the esp-rs fork:

```sh
cargo install espup --locked
espup install --targets esp32s3
. $HOME/export-esp.sh          # each shell, or source it from your profile
```

The gateway additionally needs the ESP-IDF build prerequisites (python3, cmake,
ninja, and `libudev`/`libuv` headers on Linux); `esp-idf-sys` fetches and builds
ESP-IDF itself on the first build, which takes a while and a few GB.

For flashing and the serial monitor:

```sh
cargo install espflash --locked
```

## Gateway secrets

The gateway has no filesystem and no console to be configured from, so WiFi and
Dexcom credentials are read at *compile* time via `env!` and baked into the
binary. Export them for the build; do not commit them:

```sh
export GLUCOBEACON_WIFI_SSID='...'
export GLUCOBEACON_WIFI_PASSWORD='...'
export GLUCOBEACON_DEXCOM_ACCOUNT='you@example.com'
export GLUCOBEACON_DEXCOM_PASSWORD='...'
```

A password in a flash image is not a secret from anyone holding the board — it
is `esptool.py read_flash` away. That is an accepted trade for a device with
nowhere else to put it, but it is worth knowing before this goes anywhere the
board could be picked up.

## Building

```sh
cd firmware/glucobeacon-fw-display
cargo +esp build --release
cargo +esp run --release        # flashes over USB and opens the monitor
```

Each firmware crate carries its own `rust-toolchain.toml` pinning `esp`, so a
bare `cargo` inside these directories resolves correctly. The `+esp` above is
belt and braces.

It is *not* optional when building the shared crates for a device target from
the repository root: the workspace's `rust-toolchain.toml` pins `stable`, and a
`rust-toolchain.toml` beats `rustup default`. Without `+esp` you get

```
error: the `-Z` flag is only accepted on the nightly channel of Cargo
```

which looks like a toolchain-version problem and is not one.

The board enumerates as a CP2102 USB-to-serial bridge. On Linux you will need
to be in the `dialout` group.

## Wiring

[`docs/wiring/`](../docs/wiring/) has the diagram as SVG, PDF, PNG and plain
text. All four are generated from `glucobeacon-fw-display/src/board.rs`, so
they cannot drift from the firmware; regenerate with `./tools/wiring/render.sh`
and CI fails if they are stale.

## Reconciling wiring diagram v1.0

Most of the diagram is encoded as-is: US915 on the 915 MHz board, buzzer on
GPIO15 and button LED on GPIO16 (both active high, behind MOSFET modules, both
low at boot), acknowledge button on GPIO0 active low with a pull-up, e-paper
BUSY on GPIO4 and RST on GPIO5, and a Waveshare 7.5" 800×480 panel — which
matches the layout the UI is already drawn for.

Four things were changed, all in `glucobeacon-fw-display/src/board.rs`:

1. **The e-paper SPI bus was moved off the radio.** The diagram puts e-paper
   SCK on GPIO12, MOSI on GPIO11 and MISO on GPIO13, and lists GPIO8, 9 and 14
   as free. On this board they are not: the SX1262 is soldered to GPIO8–14 and
   is not on a header, so anything sharing those pins takes the radio with it.
   The bus moved to GPIO2/3/6/7.
2. **E-paper DC and CS moved off the OLED.** The diagram's GPIO17 and GPIO18
   are the on-board OLED's I2C pins. Survivable — you would just lose the
   OLED — but it is the only display available until the e-paper works.
3. **E-paper MISO was dropped.** The Waveshare panel is write-only; its HAT
   header has no data-out line.
4. **GPIO22 and GPIO23 are not reserve pins.** Neither exists on an ESP32-S3.

`crates/glucobeacon-display/tests/heltec_board.rs` runs the board module's own
tests as part of the ordinary workspace test run, so a pin collision or an
over-budget frame fails in CI rather than on the bench.

### Strapping pins (resolved in v1.2)

The ESP32-S3 samples GPIO0, 3, 45 and 46 at reset. The acknowledge button has
moved from GPIO0 to GPIO47, and the e-Paper clock from GPIO3 to GPIO38.

GPIO0 is the one that mattered. It selects boot mode, so a button held down
through a reset left the node sitting in its serial bootloader — dark, silent,
not alarming, and giving no hint why. GPIO3 only selects the JTAG source and
was never dangerous, but a bus line has no pull at reset.

`board::STRAPPING_PINS` lists all four, and both the board tests and the
wiring generator reject anything external that lands on one.
`board::PRG_BUTTON` keeps GPIO0 available for bring-up.

### Region and range

The 915 MHz board means US915, and that has a constraint worth knowing before
the enclosure is closed. FCC 15.247 offers two routes into 902–928 MHz:
frequency hopping, which caps dwell time at 400 ms per transmission, or
digital modulation, which needs at least 500 kHz of bandwidth. A
fixed-frequency 125 kHz link is neither.

The firmware uses SF9 at 125 kHz because narrow is sensitive and sensitive is
range. At that setting a 25-byte reading is about 206 ms and the dwell ceiling
is 66 bytes, so `MAX_FRAME_FOR_DWELL` is set to 48 with a compile-time
assertion and a test that every message variant encodes under it. SF10 at
125 kHz is 412 ms and does *not* fit, which is worth knowing if you are
tempted to reach for a higher spreading factor to buy range.

If the link needs to be properly compliant rather than merely brief, the
options are 500 kHz (`Bandwidth::Bw500` — costs about 6 dB of sensitivity) or
a real hopping scheme. That is a decision about where this is deployed, not
one the code should make.

## What is not verified

Being honest about what has and has not been checked:

- **These two crates have not been compiled.** The Xtensa toolchain and the
  device crates were verified in the environment this was written in, but a
  full ESP-IDF build was not attempted there. Expect the dependency versions
  and some `esp-hal` API details to need adjustment on first build — that
  ecosystem moves quickly.
- **The pin map is from the V3 reference design, not measured.** Check
  `board.rs` against the schematic for your revision before flashing. Getting a
  LoRa pin wrong usually presents as a radio that initializes and then never
  receives anything, which is a miserable thing to debug. The collision checks
  that run in CI catch pins fighting each other, not pins that are simply the
  wrong numbers.
- **The e-paper driver is not finished.** `epaper.rs` has the UC8179 command
  set for the 7.5" V2 and handles the two traps — the controller wants 0 for
  black where the framebuffer uses 1, and BUSY is low rather than high while
  refreshing — but it has not been driven against a panel. The
  [`epd-waveshare`](https://crates.io/crates/epd-waveshare) crate has a tested
  `Epd7in5_V2` driver and is probably the better starting point.
