# Bringing up the hardware

Two Heltec boards, a panel, a few parts and a breadboard, and a repository whose
firmware has never been compiled. This is the order to do it in, and what each
step proves.

The order is not the order the block diagram suggests. It is cheapest feedback
first, and riskiest-thing-you-cannot-see last: every stage below ends with
something observable, so that when a stage fails you are debugging one new
thing rather than four at once. Resist the temptation to wire the whole node up
and flash it — the first build will not work, and a node that does nothing tells
you nothing about which of the five reasons it is.

Each stage says what it proves, what to do, and how you know it worked. If a
stage will not pass, the next one is not going to save you.

## What already works, before anything is plugged in

The whole system runs on a workstation with no radio, no panel and no Dexcom
account. Do this first, today, even if the hardware is on the desk in front of
you — it is the only way to learn what a *working* node looks like, and you
want that picture in your head before you are staring at a blank panel.

```sh
cargo test --workspace --all-features
cargo run -p glucobeacon-display -- --window        # one terminal
cargo run -p glucobeacon-gateway -- --demo          # another
```

The panel appears in a window, the demo waveform sweeps 40–260 mg/dL across
45 minutes, alarms raise and clear, Space silences them. That is the product.
Everything below is about reproducing it on the bench.

Two things worth internalising while you watch it:

- The **display node decides when to alarm**, not the gateway. A gateway you
  have not built yet is not a blocker for a display node that works — it just
  shows stale data and alarms about it, which is the correct behaviour.
- The panel only repaints when what is *visible* changes. On the bench a panel
  that is not repainting is very often a panel with nothing new to say.

## Stage 1 — The toolchain

**Proves:** you can build for the chip at all.

The ESP32-S3 is Xtensa and stock rustup cannot target it.

```sh
cargo install espup --locked
cargo install espflash --locked
espup install --targets esp32s3
. $HOME/export-esp.sh          # every shell; put it in your profile
```

On Linux, add yourself to `dialout` and log back in, or every flash is a
permissions error that reads like a missing board.

**Done when** this succeeds from the repository root:

```sh
cargo +esp build -Z build-std=core \
  -p glucobeacon-core -p glucobeacon-proto -p glucobeacon-display \
  --no-default-features --target xtensa-esp32s3-none-elf
```

That is the same command CI runs. `+esp` is load-bearing: the workspace's
`rust-toolchain.toml` pins stable, a `rust-toolchain.toml` beats `rustup
default`, and stable rejects `-Z` with an error that reads like a Cargo version
problem and is not one.

## Stage 2 — Check the pin map against your board

**Proves:** the numbers in `board.rs` describe the board you are holding.

Do this *before* stripping a single wire. The pin map in
[`glucobeacon-fw-display/src/board.rs`](../firmware/glucobeacon-fw-display/src/board.rs)
comes from the V3 reference design and has never been measured. The tests in CI
catch two peripherals on one pin, anything landing on the soldered-down SX1262,
a pin that does not exist on an S3, and anything on a strapping pin. They cannot
catch a pin that is simply the wrong number — and a wrong LoRa pin presents as a
radio that initialises cleanly and then never receives anything, which is the
worst afternoon on this whole list.

Print [`docs/wiring/display-node.pdf`](wiring/display-node.pdf), find the
schematic for your board revision, and walk the on-module rows: SX1262 on
GPIO8–14, OLED on 17/18/21, Vext on 36, LED on 35, battery sense on 1 and 37.
The external rows are your choice of free pins, so they only need to be free.

If something is wrong, **change `board.rs`, never the diagram**:

```sh
cargo test -p glucobeacon-display      # the board module's own tests
./tools/wiring/render.sh               # regenerate all four diagram formats
```

The diagram is generated from the firmware, and CI fails if the committed copy
has drifted. That arrangement exists because wiring diagram v1.0 was drawn by
hand and put the e-paper bus on top of the radio. Keep one home for the numbers.

**Done when** you have physically traced the fixed pins on your revision, and
`cargo test -p glucobeacon-display` passes with whatever you changed.

## Stage 3 — First flash, nothing wired

**Proves:** the firmware compiles, flashes, boots, and talks to you.

Antenna on the board *before* it gets power — every time, both boards. An
SX1262 transmitting into an open connector is how radios die.

```sh
cd firmware/glucobeacon-fw-display
cargo +esp run --release       # builds, flashes over USB, opens the monitor
```

Expect this **not to compile the first time**. These crates were written but
never built; `esp-hal` 1.0 and `lora-phy` 3.0 both move quickly, and the exact
constructor shapes in `main.rs` and `radio_link.rs` are the most likely thing to
have shifted. That is ordinary firmware work: read the error, check the crate's
current docs, adjust. Nothing above the HAL traits needs to change, and if you
find yourself editing anything in `crates/`, stop — that code is tested and
almost certainly not what is wrong.

Log output is already handled: `ESP_LOG = "info"` is set in the crate's
`.cargo/config.toml`, so the monitor should show the boot banner and then
`glucobeacon display node starting`.

### The first thing that will stop you

With no panel wired, `main` still calls `panel.init().expect("panel init")`, and
BUSY (GPIO4) is an input with no pull. If the floating pin settles low, `init`
waits the full 30-second timeout, returns `Timeout`, and the `expect` panics —
so the node stalls for half a minute and then reboots, over and over, before the
radio loop ever runs.

Decide how you want that to behave, because it is a product question and not
only a bench one: a display node that panics on a panel fault is dark and
silent, which is precisely the failure mode this project's design notes argue
against everywhere else. The bench-friendly and arguably correct version is to
log it and carry on, so the buzzer and the button still work when the panel does
not:

```rust
if let Err(error) = panel.init() {
    log::warn!("panel init failed, continuing without it: {error:?}");
}
```

`repaint` already logs and swallows a failed refresh, so the loop copes with
this. Your call — but make it deliberately rather than by leaving the `expect`
there and wondering why the board reboots.

**Done when** the monitor shows the starting line and the node stays up.

## Stage 4 — Buzzer, LED, button

**Proves:** GPIO, the main loop's timing, and the one interaction the product
absolutely must get right.

These are the cheap parts and they give the most feedback per wire, which is why
they come before the panel. Per the diagram: buzzer on GPIO15 and button LED on
GPIO16, both through MOSFET modules, both active high and both low at boot so
the buzzer does not shriek through every reset. The acknowledge button is on
GPIO47 to ground, active low with an internal pull-up.

Breadboard notes:

- **Common ground everywhere.** The MOSFET modules take their load supply from
  the 5 V rail; their SIG and GND must share ground with the Heltec or the gate
  never sees a real level.
- **GPIO47, not GPIO0.** GPIO0 is the boot-mode strap. A button held through a
  reset there leaves the board in its serial bootloader — dark, silent, not
  alarming, with nothing to say why. The on-board PRG button stays on GPIO0 if
  you want a spare input for bring-up.
- The SFM-27-W is an *active* buzzer: it makes its own tone, so this is a plain
  on/off pin and there is no PWM to get wrong.

You have no reading to alarm about yet, but you do not need one — the node
alarms on staleness by itself. Leave it running with nothing transmitting and
within twenty minutes it raises `Stale`, which lights the button LED. If you
would rather not wait, `AlarmConfig`'s `stale_after_mins` is the knob, and the
same value is what the sim uses.

**Done when** the LED lights on a stale feed, a press latches (`take_press`
reports presses that happened between polls, and debouncing is in
`GpioButton`), and the log says `silenced`.

## Stage 5 — The panel

**Proves:** the SPI bus, the controller command set, and the layout at full
size.

The Waveshare 7.5" HAT goes on SPI3: CS on GPIO7, SCK on GPIO38, DIN on GPIO2,
DC on GPIO6, RST on GPIO5, BUSY on GPIO4. No MISO — the panel never talks back.

Four things to get right before you blame the driver:

1. **Power.** The firmware never drives `VEXT_CONTROL` (GPIO36, active low), so
   the header's Vext rail is *off* — a panel powered from Ve will simply sit
   there. Power the HAT from a rail that is actually on, or drive GPIO36 low at
   startup. The same applies if you ever want the on-board OLED, which no
   firmware here currently uses.
2. **Slow the bus down.** `main.rs` asks SPI3 for 20 MHz. That is fine on a PCB
   and optimistic through breadboard jumpers and a ribbon; 2–4 MHz during
   prototyping removes a whole class of intermittent nonsense. Put it back later
   if you care — a full frame is 48 KB and the refresh dominates anyway.
3. **The HAT's config jumpers.** Waveshare's driver board has jumpers for
   display type and interface mode. Check them against Waveshare's wiki page for
   the 7.5" V2 in 4-wire SPI mode; wrong jumpers look exactly like bad wiring.
4. **Seat the ribbon properly.** A partly-seated FPC is the single most common
   cause of a panel that inits and never draws.

Two traps are already handled in
[`epaper.rs`](../firmware/glucobeacon-fw-display/src/epaper.rs) and worth
knowing anyway, because they change what a wrong result looks like: the
controller wants 0 for black where the framebuffer uses 1 (so a photographic
negative is a driver bug, not a layout bug), and BUSY on this panel is *low*
while it is working, the opposite of most (so a `wait` that returns instantly
and a panel that never refreshes is a polarity mistake).

That module is a documented sketch, not a tested driver. If it fights you,
[`epd-waveshare`](https://crates.io/crates/epd-waveshare) has a tested
`Epd7in5_V2` that speaks `embedded-hal`; swapping it in behind the same `Panel`
type is a smaller job than debugging a command sequence against a panel that
gives no feedback.

The best first thing to draw is the self-test, not a reading. `ui::DEMO` and
`ui::render_demo` are in the `no_std` library already — the sim's `--demo` uses
them — but the firmware does not call them yet. Walking `ui::DEMO` once at boot
lights every segment of every cell, so a dead row shows up as a gap that walks
down the panel, and the header says `self-test` so `888` can never be mistaken
for a reading.

**Done when** one pass of the self-test comes out clean, right way round, with
no missing rows.

## Stage 6 — The gateway

**Proves:** WiFi, TLS, and that your Dexcom credentials are the ones you think.

Do the credential half on your laptop, where the feedback loop is seconds rather
than an ESP-IDF rebuild:

```sh
export GLUCOBEACON_DEXCOM_ACCOUNT='you@example.com'
export GLUCOBEACON_DEXCOM_PASSWORD='...'
cargo run -p glucobeacon-gateway -- --once
```

It must be the **follower** account — the one a Share follower logs in with, not
the account on the uploading phone. And accounts are region-specific: a European
account does not exist on the US server, and logging in against the wrong one is
indistinguishable from a wrong password. Set `dexcom.region` (`us`, `ous`, `jp`)
and confirm with `--once` before any of this goes near a flash image.

Then the firmware. Budget real time for the first build: `esp-idf-sys` fetches
and builds ESP-IDF itself, which takes a while and a few GB, and needs python3,
cmake, ninja and the `libudev`/`libuv` headers.

```sh
export GLUCOBEACON_WIFI_SSID='...' GLUCOBEACON_WIFI_PASSWORD='...'
export GLUCOBEACON_DEXCOM_ACCOUNT='...' GLUCOBEACON_DEXCOM_PASSWORD='...'
cd firmware/glucobeacon-fw-gateway
cargo +esp run --release
```

Those are compile-time `env!`s baked into the binary, because the node has no
filesystem and no console. A password in a flash image is `read_flash` away for
anyone holding the board — an accepted trade, but know it before this lives
somewhere the board could be pocketed.

Also note `settings::DEXCOM_REGION` is a constant in `main.rs`, unrelated to the
radio region despite sharing the word.

**Done when** the monitor shows WiFi joined, `clock synchronised`, and
`relaying … mg/dL`. Every packet carries the time and the display node has no
RTC, so a gateway that boots without SNTP is a display that cannot tell how old
anything is — which is why it fails loudly there rather than guessing.

## Stage 7 — The link

**Proves:** the only part where both ends have to agree exactly.

Both ends must match on every field of `RadioConfig` — region, spreading factor,
bandwidth, coding rate, preamble, frequency. A mismatch in any one of them is
not a weak link, it is a silent one. The display's is `board::RADIO` and the
gateway's is `settings::RADIO`, both SF9 at 125 kHz on US915, and they are
separate declarations that you must keep identical.

Note the ordering problem for a first over-the-air test: the gateway firmware
joins WiFi and waits for SNTP *before* it touches the radio, so there is no way
to test the link alone until stage 6 passes. If you want the radio earlier —
and it is the highest-risk unknown, so wanting it earlier is reasonable —
temporarily build the gateway with `Source::Demo(DemoSource::new(...))` and skip
the WiFi and SNTP calls. That gives you a transmitter that needs no network and
no account, sending the same waveform you already watched in the sim.

Start with the two boards on the same desk, antennas attached and not touching.
Once packets flow, walk one to where it will actually live before you close
anything up.

One legal constraint comes with the narrow bandwidth, and it is worth reading
`firmware/README.md`'s "Region and range" section before the enclosure is shut:
a fixed-frequency 125 kHz link fits neither of FCC 15.247's routes into 902–928
MHz cleanly. `MAX_FRAME_FOR_DWELL` keeps every transmission brief, which is not
the same thing as compliant. Where this gets deployed is your decision to make
deliberately.

**Done when** a reading sent by the gateway appears on the panel, and the ack
comes back on a button press.

## Stage 8 — Before the enclosure

Three things the simulator cannot teach you, all worth doing while the boards
are still on a bench:

- **Schedule a periodic full refresh.** E-ink ghosts if it is only ever
  partially refreshed, and the panel has a finite number of refreshes in it.
- **Sleep the panel between refreshes.** `Panel::sleep` exists and nothing calls
  it; the controller left awake is most of the idle draw.
- **Pull the power.** The display node has no RTC and learns the wall clock from
  packet timestamps. Confirm for yourself that it comes back saying it does not
  know the time, rather than confidently showing one that is hours wrong.

## Bench habits

- Antenna before power, both boards, every time.
- Label the boards. They are physically identical and carry different firmware,
  and `GLUCOBEACON_DISPLAY_TITLE` at build time puts a name on the panel too.
- Any pin number that changes, changes in `board.rs` — then
  `cargo test -p glucobeacon-display` and `./tools/wiring/render.sh`.
- When something on the device behaves strangely, reproduce it in the sim first.
  If it reproduces, it is logic and you have tests and a debugger. If it does
  not, it is wiring, and you have narrowed it a long way.

## And the standing caveat

This is a hobby project, not a medical device, and a bench full of half-wired
prototypes is even less of one. Do not make treatment decisions from it — keep
whatever you already trust doing the job while this comes up.
