use std::sync::Arc;

use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::i2s::{config, I2sDriver, I2S0, I2S1};

use esp_idf_svc::sys::esp_sr;

const SAMPLE_RATE: u32 = 16000;
const PORT_TICK_PERIOD_MS: u32 = 1000 / esp_idf_svc::sys::configTICK_RATE_HZ;

unsafe fn afe_init() -> (
    *mut esp_sr::esp_afe_sr_iface_t,
    *mut esp_sr::esp_afe_sr_data_t,
) {
    let models = esp_sr::esp_srmodel_init("model\0".as_ptr() as *const _);
    let afe_config = esp_sr::afe_config_init(
        "M\0".as_ptr() as _,
        models,
        esp_sr::afe_type_t_AFE_TYPE_VC,
        esp_sr::afe_mode_t_AFE_MODE_HIGH_PERF,
    );
    let afe_config = afe_config.as_mut().unwrap();
    afe_config.pcm_config.total_ch_num = 1;
    afe_config.pcm_config.mic_num = 1;
    afe_config.pcm_config.ref_num = 0;
    afe_config.pcm_config.sample_rate = 16000;
    afe_config.afe_ringbuf_size = 25;

    afe_config.vad_init = true;
    // 噪声/静音段的最短持续时间（毫秒）
    afe_config.vad_min_noise_ms = 1000;
    // VAD首帧触发到语音首帧数据的延迟量
    afe_config.vad_delay_ms = 128;
    //防误触机制：需持续触发时间达到配置参数vad_min_speech_ms 才会正式触发
    // 语音段的最短持续时间（毫秒）
    afe_config.vad_min_noise_ms = 500;
    // 模式值越大，语音触发概率越高
    afe_config.vad_mode = esp_sr::vad_mode_t_VAD_MODE_1;
    afe_config.agc_init = true;

    log::info!("{afe_config:?}");

    let afe_ringbuf_size = afe_config.afe_ringbuf_size;
    log::info!("afe ringbuf size: {}", afe_ringbuf_size);

    let afe_handle = esp_sr::esp_afe_handle_from_config(afe_config);
    let afe_handle = afe_handle.as_mut().unwrap();
    let afe_data = (afe_handle.create_from_config.unwrap())(afe_config);
    let audio_chunksize = (afe_handle.get_feed_chunksize.unwrap())(afe_data);
    log::info!("audio chunksize: {}", audio_chunksize);

    esp_sr::afe_config_free(afe_config);
    (afe_handle, afe_data)
}

pub struct AFE {
    handle: *mut esp_sr::esp_afe_sr_iface_t,
    data: *mut esp_sr::esp_afe_sr_data_t,
    #[allow(unused)]
    feed_chunksize: usize,
}

unsafe impl Send for AFE {}
unsafe impl Sync for AFE {}

pub struct AFEResult {
    pub data: Vec<u8>,
    pub speech: bool,
}

impl AFE {
    pub fn new() -> Self {
        unsafe {
            let (handle, data) = afe_init();
            let feed_chunksize =
                (handle.as_mut().unwrap().get_feed_chunksize.unwrap())(data) as usize;

            AFE {
                handle,
                data,
                feed_chunksize,
            }
        }
    }
    // returns the number of bytes fed

    // 禁用AFE的vad状态
    #[allow(dead_code)]
    pub fn disable_vad(&self) {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            (afe_handle.as_ref().unwrap().disable_vad.unwrap())(afe_data);
        }
    }

    // 启用AFE的vad状态
    #[allow(dead_code)]
    pub fn enable_vad(&self) {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            (afe_handle.as_ref().unwrap().enable_vad.unwrap())(afe_data);
        }
    }

    // 重置AFE的vad状态
    #[allow(dead_code)]
    pub fn reset(&self) {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            (afe_handle.as_ref().unwrap().reset_vad.unwrap())(afe_data);
        }
    }

    // 通过ffi操作, 向afe输入音频数据
    pub fn feed(&self, data: &[u8]) -> i32 {
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            (afe_handle.as_ref().unwrap().feed.unwrap())(afe_data, data.as_ptr() as *const i16)
        }
    }
    // 这里主要是ffi操作
    pub fn fetch(&self) -> Result<AFEResult, i32> {
        // 先取出AFE的handle和data指针
        let afe_handle = self.handle;
        let afe_data = self.data;
        unsafe {
            // 先从handle里判断fetch是否存在, 它其实是一个C函数指针
            // 如果存在, 则调用fetch函数, 并将 afe_data 作为参数传入
            // 然后取出返回值, 它是一个指向 esp_afe_sr_fetch_result_t 结构体的指针
            // 再将其转换为可变引用
            let result = (afe_handle.as_ref().unwrap().fetch.unwrap())(afe_data)
                .as_mut()
                .unwrap();

            if result.ret_value != 0 {
                return Err(result.ret_value);
            }
            // 取出数据大小和vad状态
            let data_size = result.data_size;
            let vad_state = result.vad_state;
            // 根据数据大小和vad缓存大小, 创建一个足够大的Vec
            let mut data = Vec::with_capacity(data_size as usize + result.vad_cache_size as usize);
            // 如果vad缓存大小大于0, 则取出vad缓存数据, 并追加到Vec中
            // - VAD算法固有延迟：VAD无法在首帧精准触发，可能有1-3帧延迟
            // - 防误触机制：需持续触发时间达到配置参数`vad_min_speech_ms`才会正式触发
            // 为避免上述原因导致语音首字截断，AFE V2.0新增了VAD缓存机制
            if result.vad_cache_size > 0 {
                let data_ptr = result.vad_cache as *const u8;
                let data_ = std::slice::from_raw_parts(data_ptr, (result.vad_cache_size) as usize);
                data.extend_from_slice(data_);
            }
            // 如果数据大小大于0, 则取出数据, 并追加到Vec中
            if data_size > 0 {
                let data_ptr = result.data as *const u8;
                let data_ = std::slice::from_raw_parts(data_ptr, (data_size) as usize);
                data.extend_from_slice(data_);
            };
            // 判断vad状态是否为语音中
            let speech = vad_state == esp_sr::vad_state_t_VAD_SPEECH;
            // 返回数据和vad状态
            Ok(AFEResult { data, speech })
        }
    }
}

pub static WAKE_WAV: &[u8] = include_bytes!("../assets/hello_beep.wav");

pub enum AudioData {
    Hello(tokio::sync::oneshot::Sender<()>),
    SetHelloStart,
    SetHelloChunk(Vec<u8>),
    SetHelloEnd,
    Start,
    Chunk(Vec<u8>),
    End(tokio::sync::oneshot::Sender<()>),
}

pub type PlayerTx = tokio::sync::mpsc::UnboundedSender<AudioData>;
pub type PlayerRx = tokio::sync::mpsc::UnboundedReceiver<AudioData>;
pub type MicTx = tokio::sync::mpsc::Sender<crate::app::Event>;

pub async fn i2s_task_(
    i2s: I2S0,
    ws: AnyIOPin,
    sck: AnyIOPin,
    din: AnyIOPin,
    i2s1: I2S1,
    bclk: AnyIOPin,
    lrclk: AnyIOPin,
    dout: AnyIOPin,
    (tx, rx): (MicTx, PlayerRx),
) {
    // 使用arc封装AFE数据结构(通过ffi)
    let afe_handle = Arc::new(AFE::new());
    // clone 一个供线程使用
    let afe_handle_ = afe_handle.clone();
    // 启动一个线程, 该线程负责接收处理过的语音数据和vad状态, 并通过channel发送出去
    let afe_r = std::thread::spawn(|| afe_worker(afe_handle_, tx));
    // i2s player也是一个死循环, 通过i2s采集音频数据, 并喂给AFE处理, 并接收AFE的处理结果
    let r = i2s_player_(i2s, ws, sck, din, i2s1, bclk, lrclk, dout, afe_handle, rx).await;
    if let Err(e) = r {
        log::error!("Error: {}", e);
    } else {
        log::info!("I2S test completed successfully");
    }
    let r = afe_r.join().unwrap();
    if let Err(e) = r {
        log::error!("Error: {}", e);
    } else {
        log::info!("AFE worker completed successfully");
    }
}

async fn i2s_player_(
    i2s: I2S0,
    ws: AnyIOPin,
    sck: AnyIOPin,
    din: AnyIOPin,
    i2s1: I2S1,
    bclk: AnyIOPin,
    lrclk: AnyIOPin,
    dout: AnyIOPin,
    afe_handle: Arc<AFE>,
    mut rx: PlayerRx,
) -> anyhow::Result<()> {
    let i2s_config = config::StdConfig::new(
        config::Config::default().auto_clear(true),
        config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
        config::StdSlotConfig::philips_slot_default(
            config::DataBitWidth::Bits16,
            config::SlotMode::Mono,
        ),
        config::StdGpioConfig::default(),
    );
    // 创建i2s RX TX
    let mclk: Option<esp_idf_svc::hal::gpio::AnyIOPin> = None;
    let mut rx_driver = I2sDriver::new_std_rx(i2s, &i2s_config, sck, din, mclk, ws).unwrap();
    rx_driver.rx_enable()?;

    let mclk: Option<esp_idf_svc::hal::gpio::AnyIOPin> = None;
    let mut tx_driver = I2sDriver::new_std_tx(i2s1, &i2s_config, bclk, dout, mclk, lrclk).unwrap();
    tx_driver.tx_enable()?;

    // 10ms 的buffer
    let mut buf = [0u8; 2 * 160];
    let mut speaking = false;
    // 播放hello音效
    let mut hello_audio = WAKE_WAV.to_vec();
    tx_driver.write_all(&hello_audio, 100 / PORT_TICK_PERIOD_MS)?;
    log::info!("Playing hello audio, waiting for response...");
    // 创建一个死循环
    loop {
        // 如果监测到speaking
        let data = if speaking {
            // 直接通过 rx channel 提取数据
            rx.recv().await
        } else {
            // 否则通过 select!
            tokio::select! {
                Some(data) = rx.recv() =>{  // 通过 rx channel 接收数据
                    Some(data) // 将数据返回给data
                }
                _ = async {} => {
                    // 否则通过 i2s 读取数据, 并将数据喂给afe
                    for _ in 0..10{
                        let n = rx_driver.read(&mut buf, 100 / PORT_TICK_PERIOD_MS)?;
                        afe_handle.feed(&buf[..n]);
                    }
                    None
                }
            }
        };
        // 如果本轮循环接收到数据, 则进行处理
        // 否则小睡一下进入下一轮循环
        if let Some(data) = data {
            match data {
                // 如果是Hello
                AudioData::Hello(tx) => {
                    log::info!("Received hello");
                    // 通过 i2s 播放 hello 音效
                    tx_driver
                        .write_all_async(&hello_audio)
                        .await
                        .map_err(|e| anyhow::anyhow!("Error play hello: {:?}", e))?;
                    // 通过 tx channel 通知播放完成
                    let _ = tx.send(()); //使用提供的 tx 进行 ack
                    speaking = false; // 更新no speaking
                }
                // 如果是设置hello音效
                AudioData::SetHelloStart => {
                    log::info!("Received set hello start");
                    hello_audio.clear(); // 清空hello音效
                }
                AudioData::SetHelloChunk(data) => {
                    log::info!("Received set hello chunk");
                    hello_audio.extend(data); // 追加音频数据
                }
                AudioData::SetHelloEnd => {
                    log::info!("Received set hello end");
                    // 通过 i2s 播放 hello 音效
                    tx_driver
                        .write_all_async(&hello_audio)
                        .await
                        .map_err(|e| anyhow::anyhow!("Error play set hello: {:?}", e))?;
                }
                // 如果是开始(接收语音)
                AudioData::Start => {
                    log::info!("Received start");
                    speaking = true; // 更新speaking
                }
                // 如果是语音数据(段)
                AudioData::Chunk(data) => {
                    log::info!("Received audio chunk");
                    // 如果当前是speaking状态
                    if speaking {
                        // 通过i2s播放语音数据
                        tx_driver
                            .write_all_async(&data)
                            .await
                            .map_err(|e| anyhow::anyhow!("Error play audio data: {:?}", e))?;
                    }
                }
                // 如果是结束(接收完毕)
                AudioData::End(tx) => {
                    log::info!("Received end");
                    let _ = tx.send(()); //ack play done
                    speaking = false; // 更新no speaking
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    // Ok(())
}

pub async fn i2s_task(
    i2s: I2S0,
    bclk: AnyIOPin,
    din: AnyIOPin,
    dout: AnyIOPin,
    ws: AnyIOPin,
    (tx, rx): (MicTx, PlayerRx),
) {
    let afe_handle = Arc::new(AFE::new());
    let afe_handle_ = afe_handle.clone();
    let afe_r = std::thread::spawn(|| afe_worker(afe_handle_, tx));
    let r = i2s_player(i2s, bclk, din, dout, ws, afe_handle, rx).await;
    if let Err(e) = r {
        log::error!("Error: {}", e);
    } else {
        log::info!("I2S test completed successfully");
    }
    let r = afe_r.join().unwrap();
    if let Err(e) = r {
        log::error!("Error: {}", e);
    } else {
        log::info!("AFE worker completed successfully");
    }
}

async fn i2s_player(
    i2s: I2S0,
    bclk: AnyIOPin,
    din: AnyIOPin,
    dout: AnyIOPin,
    ws: AnyIOPin,
    afe_handle: Arc<AFE>,
    mut rx: PlayerRx,
) -> anyhow::Result<()> {
    log::info!("PORT_TICK_PERIOD_MS = {}", PORT_TICK_PERIOD_MS);
    let i2s_config = config::StdConfig::new(
        config::Config::default().auto_clear(true),
        config::StdClkConfig::from_sample_rate_hz(SAMPLE_RATE),
        config::StdSlotConfig::philips_slot_default(
            config::DataBitWidth::Bits16,
            config::SlotMode::Mono,
        ),
        config::StdGpioConfig::default(),
    );

    let mclk: Option<esp_idf_svc::hal::gpio::AnyIOPin> = None;

    let mut driver = I2sDriver::new_std_bidir(i2s, &i2s_config, bclk, din, dout, mclk, ws).unwrap();
    driver.tx_enable()?;
    driver.rx_enable()?;

    let mut buf = [0u8; 2 * 160];
    let mut speaking = false;

    let mut hello_audio = WAKE_WAV.to_vec();

    driver.write_all(&hello_audio, 100 / PORT_TICK_PERIOD_MS)?;
    log::info!("Playing hello audio, waiting for response...");

    loop {
        let data = if speaking {
            rx.recv().await
        } else {
            tokio::select! {
                Some(data) = rx.recv() =>{
                    Some(data)
                }
                _ = async {} => {
                    let n = driver.read(&mut buf, 100 / PORT_TICK_PERIOD_MS)?;
                    afe_handle.feed(&buf[..n]);
                    None
                }
            }
        };
        if let Some(data) = data {
            match data {
                AudioData::Hello(tx) => {
                    log::info!("Received hello");
                    driver
                        .write_all_async(&hello_audio)
                        .await
                        .map_err(|e| anyhow::anyhow!("Error play hello: {:?}", e))?;
                    log::info!("Hello audio sent, notifying");
                    let _ = tx.send(());
                    log::info!("Hello audio sent, notifying done");
                    speaking = false;
                }
                AudioData::SetHelloStart => {
                    log::info!("Received set hello start");
                    hello_audio.clear();
                }
                AudioData::SetHelloChunk(data) => {
                    log::info!("Received set hello chunk");
                    hello_audio.extend(data);
                }
                AudioData::SetHelloEnd => {
                    log::info!("Received set hello end");
                    driver
                        .write_all_async(&hello_audio)
                        .await
                        .map_err(|e| anyhow::anyhow!("Error play set hello: {:?}", e))?;
                }
                AudioData::Start => {
                    log::info!("Received start");
                    speaking = true;
                }
                AudioData::Chunk(data) => {
                    log::info!("Received audio chunk");
                    if speaking {
                        driver
                            .write_all_async(&data)
                            .await
                            .map_err(|e| anyhow::anyhow!("Error play audio data: {:?}", e))?;
                    }
                }
                AudioData::End(tx) => {
                    log::info!("Received end");
                    let _ = tx.send(());
                    speaking = false;
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
        } else {
            tokio::task::yield_now().await;
        }
    }

    // Ok(())
}

fn afe_worker(afe_handle: Arc<AFE>, tx: MicTx) -> anyhow::Result<()> {
    let mut speech = false;
    // 死循环
    loop {
        // 通过fetch获取本轮语音的数据和vad状态
        let result = afe_handle.fetch();
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
            tx.blocking_send(crate::app::Event::MicAudioChunk(result.data))
                .map_err(|_| anyhow::anyhow!("Failed to send data"))?;
            continue;
        }
        // 如果运行到这里, 首先可以说明, 本次采集的vad状态为false, 但有语音数据
        // 则可以说明是本次语音的结束段
        // 本次fetch里的语音数据没有意义? 但起码将结束状态先发送出去
        if speech {
            log::info!("Speech ended");
            tx.blocking_send(crate::app::Event::MicAudioEnd)
                .map_err(|_| anyhow::anyhow!("Failed to send data"))?;
            speech = false; //更新flag
        }
    }
}

const WELCOME_WAV: &[u8] = include_bytes!("../assets/welcome.wav");

pub fn player_welcome(
    i2s: I2S0,
    bclk: AnyIOPin,
    dout: AnyIOPin,
    lrclk: AnyIOPin,
    mclk: Option<AnyIOPin>,
    data: Option<&[u8]>,
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

    if let Some(data) = data {
        tx_driver.write_all(data, 1000).unwrap();
    } else {
        tx_driver.write_all(WELCOME_WAV, 1000).unwrap();
    }
}
