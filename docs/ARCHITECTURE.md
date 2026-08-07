# Architecture

## The shape of the thing

```
   ┌──────────────────────────┐                    ┌───────────────────────────┐
   │  gateway (WiFi)          │                    │  display node             │
   │                          │      LoRa          │                           │
   │  Dexcom Share ──▶ poll ──┼───▶  ~200 B  ─────▶┼── history ──▶ 7" e-ink    │
   │                          │      packets       │       │                   │
   │  no alarm state          │                    │       ├──▶ piezo buzzer   │
   │                          │◀───  acks (opt) ───┼──     └──▶ button + LED   │
   └──────────────────────────┘                    └───────────────────────────┘
```

Data flows one way. The reverse channel carries acknowledgements and nothing
the system depends on.

## Why the display node owns the alarms

The tempting design is to have the gateway decide "this is a low" and send an
alarm command. It is wrong, because it makes every gateway failure silent. If
the gateway crashes, loses WiFi, or has its Dexcom password rotated, no alarm
command is ever sent — and a display waiting to be told to alarm sits there
looking perfectly healthy.

So the display node holds the alarm state machine and decides for itself, from
readings and from the passage of time. A gateway that stops talking becomes a
[`Stale`](../crates/glucobeacon-core/src/alarm.rs) alarm within twenty minutes,
which is the correct behaviour and requires no cooperation from the end that
broke.

The gateway still owns the *policy* — thresholds, silence duration — and pushes
it to the display on startup and every heartbeat. Configuration lives in one
place; the decision lives at the end that can still act on it.

## Crates

### `glucobeacon-core` — shared vocabulary

`no_std`, no allocation. Holds `Glucose`, `Trend`, `Reading`, `Timestamp`, a
fixed-capacity `History`, and the `AlarmEngine`.

The alarm engine is a pure state machine: no clock, no I/O. It takes `now` and
the newest reading and reports transitions. That makes the parts that are
genuinely hard to get right — hysteresis, silence, escalation, staleness — a
matter of arithmetic that unit tests can pin down completely.

Three rules in it are worth knowing:

- **Hysteresis.** An alarm clears only once the reading has come back past its
  threshold by a margin. Without it, a value parked on 70 mg/dL flips the buzzer
  on and off every five minutes.
- **Escalation breaks silence.** Silencing a low silences *that* alarm. If it
  becomes an urgent low, the silence is discarded — the user silenced a low, not
  what it turned into.
- **Losing signal while low keeps the low.** When data goes stale, the engine
  keeps whichever verdict is more severe: the staleness, or the last known band.
  "Urgent low, and now we have lost contact" must not quietly demote to
  "no data".

### `glucobeacon-proto` — the wire

`no_std`. Frame layout, message types, and the `Link` trait.

A frame is magic, version, length, a postcard-encoded payload, and a CRC-16 —
six bytes of overhead, with a reading fitting in about twenty-five bytes total.
The design assumption is a radio with a ~200 byte payload budget, no MAC-layer
acknowledgement, and real packet loss. Every message is therefore self-contained
and idempotent: losing one costs five minutes of staleness, not a corrupted
state machine.

`Message` variants are append-only within a protocol version. Postcard encodes
an enum as its variant index, so reordering them silently changes the meaning of
every frame on the air; a test pins the current indices.

`Link` abstracts the transport. A LoRa driver, a UDP socket, and a test double
all satisfy it, which is what lets both nodes run against each other on one
machine with no hardware.

### `glucobeacon-dexcom` — the upstream

Share is the private API behind Dexcom's "Followers" feature. It is undocumented
and unversioned, and this client is written to survive its quirks: a two-step
login, `/Date(1700000000000-0500)/` timestamps, a `Trend` field that is sometimes
a number and sometimes a string, and application errors reported with an HTTP 500
and a JSON body. Sessions expire after a few hours and are renewed transparently.

Errors are classified into what the caller should *do*: a network failure and a
wrong password both fail the poll, but only one of them is worth retrying, and
the display says something different for each.

### `glucobeacon-gateway`

Polls, relays, and nothing else.

Polling on a fixed timer is the obvious approach and the wrong one: the sensor
produces a reading every five minutes on *its* phase, so a fixed timer spends
most requests asking for data that already arrived and delivers each new reading
up to five minutes late. `PollSchedule` instead tracks the phase of the last
reading and aims just past when the next is due, with a floor to prevent hot
looping and exponential backoff on failure.

The radio runs on its own thread and talks to the async gateway over channels.
Radio drivers are blocking — SPI transfers, busy-waiting on a DIO pin — and HTTP
is async; rather than contort either, they meet at a queue.

### `glucobeacon-display`

`app` is the logic and has no I/O in it: packets and time go in, redraw
decisions and buzzer patterns come out. `hal` is the hardware as traits. `sim`
implements those traits against a workstation.

Two things here are shaped by the panel:

**Refresh is expensive.** A full refresh of a 7-inch e-ink panel takes about a
second and visibly flashes the whole screen. So the node reduces what is
*visible* to a fingerprint — the value, the arrow, the whole-minute age, the
alarm, the thresholds — and repaints only when that changes. A retransmitted
reading, or thirty seconds passing inside the same displayed minute, is not a
reason to spend a refresh.

**There is no font.** Digits readable across a dark room are ~190 px tall, and a
bitmap font that size is a lot of flash on a node that has none to spare. The
digits are drawn as seven segments of rectangles and the arrow as triangles, so
both scale for free and cost nothing.

The node also has no RTC and no NTP. It learns the wall clock from the `sent_at`
stamp on every packet and carries it forward on a monotonic uptime counter.
Before the first packet it does not know what time it is and says so, rather than
showing a confident "2 min ago" that is hours wrong.

## Target hardware

Heltec WiFi LoRa 32 V3, one per end: an ESP32-S3FN8 with an SX1262 radio,
8 MB of SiP flash, a CP2102 USB-serial bridge, and a LiPo charger.

### Toolchain

The ESP32-S3 is Xtensa, so stock rustup will not build for it. The esp-rs fork
is required:

```sh
cargo install espup --locked
espup install --targets esp32s3
. $HOME/export-esp.sh
```

| | Target |
| --- | --- |
| Display node (`esp-hal`, `no_std`) | `xtensa-esp32s3-none-elf` |
| Gateway (`esp-idf-svc`, `std`) | `xtensa-esp32s3-espidf` |

CI builds the device crates for `xtensa-esp32s3-none-elf` via the
`esp-rs/xtensa-toolchain` action, and *also* for
`riscv32imc-unknown-none-elf`. The second is not a shipping target — it is a
stricter check that a stock toolchain can run: RISC-V `imc` has no atomic
compare-and-swap, so anything that quietly depends on one fails there rather
than on the bench.

### `std` on one end, `no_std` on the other

The two ends want different things, and the crate split already allows it:

- **Gateway: `std`, via `esp-idf-svc`.** It needs WiFi and TLS. ESP-IDF brings
  both, along with a working mbedTLS and a socket layer. Doing HTTPS from
  `no_std` on this chip is possible and not worth it.
- **Display node: `no_std`, via `esp-hal`.** It needs SPI, GPIO, and a timer.
  Everything in `glucobeacon-display` except the `sim` module is `no_std` and
  allocation-free, so the whole node — alarm engine, layout, glyphs,
  framebuffer — cross-compiles for bare metal today.

`glucobeacon-dexcom` is written against an `HttpTransport` trait rather than
against `reqwest` for this reason: an ESP-IDF build implements the trait over
`esp_idf_svc::http::client::EspHttpConnection` and turns the `reqwest-transport`
feature off, so the device is not carrying two TLS stacks to reach one JSON API.

### Memory

The ESP32-S3FN8 has 512 KB of SRAM. The panel framebuffer is the one allocation
big enough to matter:

| | Bytes |
| --- | --- |
| 800×480 at one byte per pixel | 384 000 — does not fit |
| 800×480 packed, one bit per pixel | 48 000 |

Hence `framebuffer::FrameBuffer`, which is packed, and `PanelBuffer`, which is
sized for the panel. 48 KB out of 512 KB is comfortable, but it is still far
too much for the stack — the firmware puts it in a `StaticCell`. Everything
else is small: the reading history is 36 entries, and a frame on the wire is
about 25 bytes.

This module has no PSRAM, so 512 KB is the whole budget. The gateway is the
tighter of the two ends, because ESP-IDF's WiFi and TLS stacks want a good
part of it.

If DRAM turns out to be tight, the fallback is a controller that supports
partial refresh and a windowed buffer, updating only the region that changed.
That is also why the framebuffer keeps its row padding canonical: a
partial-refresh driver decides what to send by diffing the previous buffer
against the new one, and padding bits that varied with how an image was drawn
would show up as spurious changes.

## Bringing up hardware

Nothing above the traits changes. What is needed:

1. A `Link` implementation for the SX1262, via `lora-phy`. Both ends must agree
   on every field of `proto::radio::RadioConfig` exactly; a mismatch in any one
   of them is not a weak link but a silent one. The module covers 863–928 MHz,
   which spans both the EU and US bands, so nothing in the hardware stops you
   transmitting on the wrong one — see `radio::RadioConfig::validate`.
2. `Panel` plus `DrawTarget<Color = BinaryColor>` for the e-ink controller.
   `FrameBuffer` already implements `DrawTarget`, so this is `flush` pushing
   `as_bytes()` over SPI.
3. `Buzzer`, `Indicator`, and `Button` for the piezo, LED, and switch. The button
   must latch its press: `take_press` reports a press that happened between
   polls, because the one thing a user must always be able to do is silence an
   alarm. Debouncing belongs in the implementation.
4. An `HttpTransport` for the gateway over ESP-IDF's HTTP client.

`glucobeacon-display::sim` is a worked example of 2 and 3, and
`glucobeacon-dexcom::reqwest_transport` of 4.

Two things the simulator does not model and the hardware will need: the e-ink
panel has a finite refresh budget and ghosts if it is only ever partially
refreshed, so a periodic full refresh is worth scheduling; and the display node
is the one that must survive a power cut without a wall clock, which it already
handles by learning the time from packets rather than assuming it.
