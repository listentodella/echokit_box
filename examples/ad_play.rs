use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_svc::hal::i2s::{config, I2sDriver};

const SAMPLE_RATE: u32 = 16000;
static WAVE_DATA: &'static [u8] = include_bytes!("../assets/hello.wav");

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

    let bclk = peripherals.pins.gpio15;
    let dout = peripherals.pins.gpio7;
    let lrclk = peripherals.pins.gpio16;
    let mclk: Option<esp_idf_svc::hal::gpio::AnyIOPin> = None;

    let mut tx_driver =
        I2sDriver::new_std_tx(peripherals.i2s1, &i2s_config, bclk, dout, mclk, lrclk).unwrap();
    tx_driver.tx_enable()?;

    tx_driver.write_all(WAVE_DATA, 1000)?;

    Ok(())
}
