use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_svc::hal::i2s::{config, I2sDriver};
use esp_idf_svc::io::Read;

const SAMPLE_RATE: u32 = 16000;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let _sysloop = EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;

    let i2s_config = config::StdConfig::new(
        config::Config::default().auto_clear(true),
        config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
        config::StdSlotConfig::philips_slot_default(
            config::DataBitWidth::Bits16,
            config::SlotMode::Mono,
        ),
        config::StdGpioConfig::default(),
    );

    let din = peripherals.pins.gpio6;
    let ws = peripherals.pins.gpio4;

    let bclk = peripherals.pins.gpio15;
    let mclk: Option<esp_idf_svc::hal::gpio::AnyIOPin> = None;

    let mut rx_driver =
        I2sDriver::new_std_rx(peripherals.i2s0, &i2s_config, bclk, din, mclk, ws).unwrap();
    rx_driver.rx_enable()?;
    // prepare a buffer for voice data - 5s under the sample rate
    let mut buffer = vec![0u8; 5 * SAMPLE_RATE as usize * 2];
    // record data into the buffer
    rx_driver.read_exact(&mut buffer)?;

    log::info!("5s voice data recorded, you can play the buffer");
    //log::info!("{:?}", buffer);

    Ok(())
}
