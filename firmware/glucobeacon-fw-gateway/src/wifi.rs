//! Joining the access point.
//!
//! Blocking and simple: the gateway has nothing useful to do until it is on
//! the network, and the display node already treats a silent gateway as stale
//! data rather than as everything being fine.

use anyhow::{Context, Result, bail};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

/// Connects to `ssid` and returns the interface, which must be kept alive.
pub fn connect(
    modem: Modem,
    event_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    password: &str,
) -> Result<BlockingWifi<EspWifi<'static>>> {
    if ssid.is_empty() {
        bail!("no WiFi SSID compiled in");
    }

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, event_loop.clone(), Some(nvs)).context("creating the wifi driver")?,
        event_loop,
    )
    .context("wrapping wifi")?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().map_err(|_| anyhow::anyhow!("ssid too long"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method: if password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    }))
    .context("configuring wifi")?;

    wifi.start().context("starting wifi")?;
    wifi.connect().context("associating")?;
    wifi.wait_netif_up().context("waiting for DHCP")?;

    let info = wifi.wifi().sta_netif().get_ip_info().context("ip info")?;
    log::info!("wifi up, address {}", info.ip);
    Ok(wifi)
}
