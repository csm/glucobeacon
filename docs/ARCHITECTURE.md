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

## Bringing up hardware

Nothing above the traits changes. What is needed:

1. A `Link` implementation for the LoRa module — `send_frame` and `recv_frame`
   over SPI, with `recv_frame` returning `Ok(None)` on timeout.
2. `Panel` plus `DrawTarget<Color = BinaryColor>` for the e-ink controller.
3. `Buzzer`, `Indicator`, and `Button` for the piezo, LED, and switch. The button
   must latch its press: `take_press` reports a press that happened between
   polls, because the one thing a user must always be able to do is silence an
   alarm.

`glucobeacon-display::sim` is a worked example of all of these.
