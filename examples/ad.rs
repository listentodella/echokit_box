use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_svc::hal::i2s::{config, I2sDriver};
use esp_idf_svc::io::asynch::Read;
const SAMPLE_RATE: u32 = 16000;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let _sysloop = EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;

    let mut button = esp_idf_svc::hal::gpio::PinDriver::input(peripherals.pins.gpio0)?;
    button.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::PosEdge)?;

    let (evt_tx, mut evt_rx) = tokio::sync::mpsc::channel(64);
    let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();

    let i2s_record_task = async move {
        log::info!("record task started");
        let i2s = peripherals.i2s0;
        let ws = peripherals.pins.gpio4;
        let bclk = peripherals.pins.gpio5;
        let din = peripherals.pins.gpio6;
        let mclk: Option<esp_idf_svc::hal::gpio::AnyIOPin> = None;
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
        loop {
            match evt_rx.recv().await {
                Some(v) => {
                    log::info!("start recording: {}?", v);
                    if v == 1 {
                        // prepare a buffer for voice data - max 5s under the sample rate
                        let mut buffer = vec![0u8; 5 * SAMPLE_RATE as usize * 2];
                        // record data into the buffer
                        rx_driver.read_exact(&mut buffer).await.unwrap();
                        tx1.send(buffer).unwrap();
                    }
                }
                None => {
                    log::info!("Event channel closed");
                    break;
                }
            }
        }
    };

    let i2s_play_task = async move {
        log::info!("play task started");
        let i2s = peripherals.i2s1;
        let dout = peripherals.pins.gpio7;
        let bclk = peripherals.pins.gpio15;
        let lrclk = peripherals.pins.gpio16;
        let mclk: Option<esp_idf_svc::hal::gpio::AnyIOPin> = None;
        let i2s_config = config::StdConfig::new(
            config::Config::default().auto_clear(true),
            config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
            config::StdSlotConfig::philips_slot_default(
                config::DataBitWidth::Bits16,
                config::SlotMode::Mono,
            ),
            config::StdGpioConfig::default(),
        );

        let mut tx_driver =
            I2sDriver::new_std_tx(i2s, &i2s_config, bclk, dout, mclk, lrclk).unwrap();
        tx_driver.tx_enable().unwrap();

        loop {
            // match evt_rx.recv().await {
            //     Some(v) => {
            //         log::info!("stop recording: {}", v);
            //         break;
            //     }
            //     None => {
            //         log::info!("Event channel closed");
            //         break;
            //     }
            // }
            //FIXME: 这里并非阻塞的, 如果rx1里没有数据, 第一次会等, 后续不会阻塞等待, 导致不停走None分支
            let samples = rx1.recv().await;
            if let Some(samples) = samples {
                tx_driver.write_all(&samples, 1000).unwrap();
            } else {
                log::info!("play default samples");
                // let default_samples = include_bytes!("../assets/hello.wav");
                // tx_driver.write_all(default_samples, 1000).unwrap();
            }
        }
    };

    let b = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    b.spawn(i2s_record_task);
    b.spawn(i2s_play_task);

    b.block_on(async move {
        loop {
            let _ = button.wait_for_falling_edge().await;
            log::info!("Button pressed!");
            evt_tx.send(1u8).await.unwrap();
            let _ = button.wait_for_rising_edge().await;
            evt_tx.send(0u8).await.unwrap();
            log::info!("Button released!");
        }
    });

    Ok(())
}
