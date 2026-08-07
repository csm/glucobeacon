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

The board enumerates as a CP2102 USB-to-serial bridge. On Linux you will need
to be in the `dialout` group.

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
  receives anything, which is a miserable thing to debug.
- **The region defaults to EU868.** The module covers 863–928 MHz, so nothing
  in the hardware stops you transmitting on the wrong band for where you are.
  Set `REGION` in `board.rs` before flashing, and see
  `glucobeacon-proto::radio` for why the US preset uses 500 kHz.
