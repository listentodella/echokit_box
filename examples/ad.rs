use esp_idf_svc::eventloop::EspSystemEventLoop;

use echokit::audio;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = esp_idf_svc::hal::prelude::Peripherals::take().unwrap();
    let _sysloop = EspSystemEventLoop::take()?;
    let _fs = esp_idf_svc::io::vfs::MountedEventfs::mount(20)?;

    let mut button = esp_idf_svc::hal::gpio::PinDriver::input(peripherals.pins.gpio0)?;
    button.set_pull(esp_idf_svc::hal::gpio::Pull::Up)?;
    button.set_interrupt_type(esp_idf_svc::hal::gpio::InterruptType::PosEdge)?;

    let (evt_tx, evt_rx) = tokio::sync::mpsc::channel(64);
    let (tx1, rx1) = tokio::sync::mpsc::unbounded_channel();

    let i2s_task = {
        let sck = peripherals.pins.gpio5;
        let din = peripherals.pins.gpio6;
        let dout = peripherals.pins.gpio7;
        let ws = peripherals.pins.gpio4;
        let bclk = peripherals.pins.gpio15;
        let lrclk = peripherals.pins.gpio16;

        audio::i2s_task_(
            peripherals.i2s0,
            ws.into(),
            sck.into(),
            din.into(),
            peripherals.i2s1,
            bclk.into(),
            lrclk.into(),
            dout.into(),
            (evt_tx.clone(), rx1),
        )
    };

    let b = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    b.spawn(i2s_task);

    b.block_on(async move {
        loop {
            let _ = button.wait_for_falling_edge().await;
            log::info!("Button pressed!");
        }
    });

    Ok(())
}
