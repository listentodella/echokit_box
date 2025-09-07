use esp_idf_svc::eventloop::EspSystemEventLoop;
use std::sync::Arc;

use esp_idf_svc::hal::i2s::{config, I2sDriver};
use esp_idf_svc::io::asynch::Read;

use echokit::audio::{AFEResult, AFE};
const SAMPLE_RATE: u32 = 16000;
const PORT_TICK_PERIOD_MS: u32 = 1000 / esp_idf_svc::sys::configTICK_RATE_HZ;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let _sysloop = EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;

    // let (evt_tx, mut evt_rx) = tokio::sync::mpsc::channel(64);
    // let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();
    log::info!(
        ".....................................VAD EXAMPLE....................................."
    );
    let afe_handle = Arc::new(AFE::new());
    let afe_handle_1 = afe_handle.clone();
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

        afe_handle.enable_vad();

        let mut rx_driver = I2sDriver::new_std_rx(i2s, &i2s_config, bclk, din, mclk, ws).unwrap();
        rx_driver.rx_enable().unwrap();
        // prepare a buffer for voice data - 5s under the sample rate
        let mut buffer = vec![0u8; 5 * SAMPLE_RATE as usize * 2];

        loop {
            // rx_driver.read_async(&mut buffer).await.unwrap();
            for i in 0..5 {
                let n = rx_driver
                    .read(&mut buffer, 100 / PORT_TICK_PERIOD_MS)
                    .unwrap();
                log::info!(
                    "................AFE FEED {i}.... buffer empty = {}",
                    buffer.is_empty()
                );
                afe_handle.feed(&buffer[..n]);
            }
            log::info!("................AFE FEED DONE................");
            // afe_handle.feed(&buffer);
            // let result = afe_handle.fetch();
            // match result {
            //     Ok(result) => {
            //         if result.speech {
            //             log::info!("speech detected, len: {}", result.data.len());
            //         } else {
            //             log::info!("no speech, len: {}", result.data.len());
            //         }
            //     }
            //     Err(e) => {
            //         log::error!("AFE fetch error: {}", e);
            //     }
            // }
        }
    };
    let b = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    b.spawn(i2s_record_task);

    b.block_on(async move {
        let mut speech = false;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            // 通过fetch获取本轮语音的数据和vad状态
            let result = afe_handle_1.fetch();
            if let Err(_e) = &result {
                continue;
            }
            let result = result.unwrap();
            // 如果没有数据, 则继续下一轮fetch
            if result.data.is_empty() {
                continue;
            }
            // 运行到这里, 首先可以说明, 有语音数据
            // 然后, 如果vad状态为true, 则说明语音仍然在进行
            // 先将已采集到的数据通过channel发送出去
            // 然后进行下一轮fetch
            if result.speech {
                speech = true; //更新flag
                log::debug!("Speech detected, sending {} bytes", result.data.len());
                continue;
            }
            // 如果运行到这里, 首先可以说明, 本次采集的vad状态为false, 但有语音数据
            // 则可以说明是本次语音的结束段
            // 本次fetch里的语音数据没有意义? 但起码将结束状态先发送出去
            if speech {
                log::info!("Speech ended");
                speech = false; //更新flag
            }
        }
    });
    Ok(())
}
