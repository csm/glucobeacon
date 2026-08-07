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
  glucobeacon-dexcom    Dexcom Share client                        (std, gateway only)
  glucobeacon-gateway   the WiFi node                              (binary)
  glucobeacon-display   the e-ink node                             (binary)
```

`core` and `proto` are `no_std` and allocation-free, and CI cross-compiles them
for `thumbv7em-none-eabihf` to keep them that way. Both nodes depend on both, so
neither end can drift on what a reading means or when it should alarm.

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

## Status

The domain logic, wire protocol, Dexcom client, and both node applications are
written and tested. What is not here yet is the hardware: the display node runs
as a workstation simulator, with the panel, buzzer, LED, and button behind the
traits in `glucobeacon-display::hal`. Bringing up real hardware means
implementing those traits and the `Link` trait for a LoRa driver — nothing above
them changes.

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features
cargo build -p glucobeacon-core -p glucobeacon-proto \
  --no-default-features --target thumbv7em-none-eabihf
```

## Safety

This is a hobby project and not a medical device. Do not make treatment
decisions from it. Dexcom Share is a private API with no stability guarantees,
and a radio link that drops packets is a link that will, sooner or later, not
tell you something you wanted to know.
