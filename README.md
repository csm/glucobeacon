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
except its simulator and host-window modules. CI cross-compiles them for a bare-metal target to
keep it that way. Both nodes depend on both shared crates, so neither end can
drift on what a reading means or when it should alarm.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit and why.

## Running it

Neither radio hardware nor a Dexcom account is needed to see the whole system
work. The gateway can synthesize a glucose waveform, and both nodes can speak
the protocol over UDP instead of over the air.

In one terminal:

```sh
cargo run -p glucobeacon-display -- --window
```

In another:

```sh
cargo run -p glucobeacon-gateway -- --demo
```

`--window` opens a window on this machine showing the panel, which is the
quickest way to work on the layout with no hardware in front of you. It updates
only when the panel refreshes, because that is the only time real e-ink changes.
Space or Enter over the window silences an alarm, as does Enter in the terminal;
Esc closes it and stops the node. `--window-scale 2` opens it magnified, and the
window is resizable either way.

The window needs no system packages — no SDL2, no Homebrew — and works on macOS,
Linux and Windows. Leave `--window` off and the sim runs headless, which is what
CI and a container want:

```sh
cargo run -p glucobeacon-display -- --panel panel.pbm
```

Either way the display writes `--panel` (default `glucobeacon-panel.pbm`) on
every refresh — a netpbm bitmap any image viewer will open. The buzzer and LED
appear as log lines.

The demo waveform sweeps 40–260 mg/dL over 45 minutes, so it crosses every alarm
threshold in both directions while you watch.

The display has a `--demo` of its own, and it is a different thing: where the
gateway's invents data, the display's draws none at all. It cycles the panel
through every digit — each one filling all three cells — and then `HI` and `LO`,
holding each for `--demo-dwell` seconds (2 by default) before carrying on into
normal operation:

```sh
cargo run -p glucobeacon-display -- --window --demo
```

It is the quick way to look at the glyphs, and on real hardware it doubles as a
panel self-test: one pass lights every segment of every cell, so a dead row
shows up as a gap that walks down the screen. The header says `self-test` while
it runs, because a panel showing `888` and nothing else should never be mistaken
for a reading. It is opt-in and runs once rather than looping — each frame is a
full refresh, and an e-ink panel has a finite number of those.

The panel's top-left corner says `GLUCOBEACON` unless it was built with a name
of its own, which is how a household with two panels tells them apart:

```sh
GLUCOBEACON_DISPLAY_TITLE="ROWAN" cargo run -p glucobeacon-display -- --window
```

That is a *compile*-time variable, not a runtime one — the real display node has
no filesystem and no console to read a setting from, so the choice belongs to
whoever builds the binary. See [firmware/README.md](firmware/README.md#display-name).

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
from the root does not require the Xtensa toolchain. The wiring is in
[`docs/wiring/`](docs/wiring/) as SVG, PDF, PNG and plain text — all generated
from the firmware's own pin map, so the diagram cannot disagree with the code.

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
`Link`, and the panel can be watched live in a host window.

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features

# Everything that will run on the device, for the real target.
#
# `+esp` is required, not decorative: rust-toolchain.toml pins this workspace
# to stable, and that file beats `rustup default`. A bare `cargo` here resolves
# to stable, which cannot target Xtensa and rejects `-Z`. And `-Z build-std` is
# required too — the esp toolchain ships no precompiled `core` for a bare-metal
# Xtensa target.
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
