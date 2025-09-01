use anyhow;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        log::info!("hello from std Thread~~");
    });

    Ok(())
}
