use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_svc::hal::adc::AdcChannels;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::i2s::{config, I2sDriver, I2S0, I2S1};
use esp_idf_svc::io::Read;
const SAMPLE_RATE: u32 = 16000;

fn record(
    i2s: I2S0,
    din: AnyIOPin,
    ws: AnyIOPin,
    bclk: AnyIOPin,
    mclk: Option<AnyIOPin>,
) -> Vec<u8> {
    let i2s_config = config::StdConfig::new(
        config::Config::default().auto_clear(true),
        config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
        config::StdSlotConfig::philips_slot_default(
            config::DataBitWidth::Bits16,
            config::SlotMode::Mono,
        ),
        config::StdGpioConfig::default(),
    );

    let mut rx_driver = I2sDriver::new_std_rx(i2s, &i2s_config, bclk, din, mclk, ws).unwrap();
    rx_driver.rx_enable().unwrap();
    // prepare a buffer for voice data - 5s under the sample rate
    let mut buffer = vec![0u8; 5 * SAMPLE_RATE as usize * 2];
    // record data into the buffer
    rx_driver.read_exact(&mut buffer).unwrap();

    buffer
}

fn play(
    i2s: I2S1,
    bclk: AnyIOPin,
    dout: AnyIOPin,
    lrclk: AnyIOPin,
    mclk: Option<AnyIOPin>,
    samples: Option<&[u8]>,
) {
    let i2s_config = config::StdConfig::new(
        config::Config::default().auto_clear(true),
        config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
        config::StdSlotConfig::philips_slot_default(
            config::DataBitWidth::Bits16,
            config::SlotMode::Mono,
        ),
        config::StdGpioConfig::default(),
    );

    let mut tx_driver = I2sDriver::new_std_tx(i2s, &i2s_config, bclk, dout, mclk, lrclk).unwrap();
    tx_driver.tx_enable().unwrap();

    if let Some(samples) = samples {
        tx_driver.write_all(samples, 1000).unwrap();
    } else {
        log::info!("play default samples");
        let default_samples = include_bytes!("../assets/hello.wav");
        tx_driver.write_all(default_samples, 1000).unwrap();
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let _sysloop = EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;

    let ws = peripherals.pins.gpio4;
    let bclk = peripherals.pins.gpio5;
    let din = peripherals.pins.gpio6;
    let samples = record(peripherals.i2s0, din.into(), ws.into(), bclk.into(), None);
    log::info!("record done, samples len {}", samples.len());

    let dout = peripherals.pins.gpio7;
    let bclk = peripherals.pins.gpio15;
    let lrclk = peripherals.pins.gpio16;
    play(
        peripherals.i2s1,
        bclk.into(),
        dout.into(),
        lrclk.into(),
        None,
        Some(&samples),
    );

    Ok(())
}
