# glucobeacon

Remote glucose monitoring system.

A pair of nodes joined by a LoRa radio link:

- the **gateway** has WiFi. It polls the Dexcom Share API for CGM readings and
  relays each new one over the radio.
- the **display node** has no network. It drives a 7-inch e-ink panel, a piezo
  buzzer, and a lit button for silencing alarms.

The display node is the one that decides when to make noise. That is deliberate:
a gateway that crashes, loses WiFi, or gets its Dexcom password changed then
looks like exactly what it is — stale data, which alarms — instead of looking
like everything is fine.

## Layout

```
crates/
  glucobeacon-core      domain types and the alarm state machine   (no_std)
  glucobeacon-proto     the wire protocol and the Link trait       (no_std)
  glucobeacon-dexcom    Dexcom Share client                        (gateway only)
  glucobeacon-gateway   the WiFi node                              (lib + binary)
  glucobeacon-display   the e-ink node                             (lib + binary, no_std)
```

`core` and `proto` are `no_std` and allocation-free, and so is all of `display`
except its simulator module. CI cross-compiles them for a bare-metal target to
keep it that way. Both nodes depend on both shared crates, so neither end can
drift on what a reading means or when it should alarm.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit and why.

## Running it

Neither radio hardware nor a Dexcom account is needed to see the whole system
work. The gateway can synthesize a glucose waveform, and both nodes can speak
the protocol over UDP instead of over the air.

In one terminal:

```sh
cargo run -p glucobeacon-display -- --panel panel.pbm
```

In another:

```sh
cargo run -p glucobeacon-gateway -- --demo
```

The display writes `panel.pbm` on every refresh — open it in any image viewer to
see what the e-ink would show. The buzzer and LED appear as log lines. Press
Enter in the display's terminal to silence an alarm.

The demo waveform sweeps 40–260 mg/dL over 45 minutes, so it crosses every alarm
threshold in both directions while you watch.

## Against a real Dexcom account

The gateway needs a Dexcom **follower** account — the one a Share follower logs
in with, not the account on the phone doing the uploading.

```sh
export GLUCOBEACON_DEXCOM_ACCOUNT='you@example.com'
export GLUCOBEACON_DEXCOM_PASSWORD='...'
cargo run -p glucobeacon-gateway -- --once          # check it works
cargo run -p glucobeacon-gateway -- --config glucobeacon.toml
```

The password is read only from the environment and is deliberately not a config
file field. Accounts are region-specific: a European account does not exist on
the US server, and logging in against the wrong one is indistinguishable from a
wrong password. Set `dexcom.region` (`us`, `ous`, or `jp`) accordingly.

See [glucobeacon.example.toml](glucobeacon.example.toml) for the settings.

## Target hardware

Heltec WiFi LoRa 32 V3 at each end: an ESP32-S3FN8 with an SX1262 radio, 8 MB of
flash, and 512 KB of SRAM.

The ESP32-S3 is Xtensa, so it needs `espup` — an esp-rs fork of rustc that stock
rustup cannot install. The gateway builds against `esp-idf-svc` for `std`,
because it needs WiFi and TLS and ESP-IDF has both. The display node builds
against `esp-hal` for `no_std`, and all of its logic already compiles for
`xtensa-esp32s3-none-elf` today.

Firmware lives in [`firmware/`](firmware/), outside the workspace, so building
from the root does not require the Xtensa toolchain.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the memory budget, the
radio parameters, and what each hardware trait needs.

## Status

The domain logic, wire protocol, Dexcom client, radio parameters, and both node
applications are written and tested, and everything destined for the device
compiles for the ESP32-S3.

The firmware crates are written but **not yet compiled** — see
[firmware/README.md](firmware/README.md) for exactly what that means. The
display node also runs as a workstation simulator, with the panel, buzzer, LED,
and button behind the traits in `glucobeacon-display::hal` and the radio behind
`Link`.

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features

# Everything that will run on the device, for the real target.
cargo +esp build -Z build-std=core \
  -p glucobeacon-core -p glucobeacon-proto -p glucobeacon-display \
  --no-default-features --target xtensa-esp32s3-none-elf

# The Dexcom client without reqwest, as an ESP-IDF build would use it.
cargo build -p glucobeacon-dexcom --no-default-features
```

## Safety

This is a hobby project and not a medical device. Do not make treatment
decisions from it. Dexcom Share is a private API with no stability guarantees,
and a radio link that drops packets is a link that will, sooner or later, not
tell you something you wanted to know.
