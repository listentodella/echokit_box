use tokio::sync::mpsc;
use tokio_websockets::Message;

use crate::{
    audio::{self, AudioData},
    protocol::ServerEvent,
    ws::Server,
};

#[derive(Debug)]
pub enum Event {
    Event(&'static str),
    ServerEvent(ServerEvent),
    MicAudioChunk(Vec<u8>),
    MicAudioEnd,
}

#[allow(dead_code)]
impl Event {
    pub const GAIA: &'static str = "gaia";
    pub const NO: &'static str = "no";
    pub const YES: &'static str = "yes";
    pub const NOISE: &'static str = "noise";
    pub const RESET: &'static str = "reset";
    pub const UNKNOWN: &'static str = "unknown";
    pub const K0: &'static str = "k0";
    pub const K0_: &'static str = "k0_";

    pub const K1: &'static str = "k1";
    pub const K2: &'static str = "k2";
}
// 监听 evt_rx(from 麦克风) 和 server(from服务器) 的事件
async fn select_evt(evt_rx: &mut mpsc::Receiver<Event>, server: &mut Server) -> Option<Event> {
    tokio::select! {
        Some(evt) = evt_rx.recv() => {
            match &evt {
                Event::Event(_)=>{
                    log::info!("Received event: {:?}", evt);
                },
                Event::MicAudioEnd=>{
                    log::info!("Received MicAudioEnd");
                },
                Event::MicAudioChunk(data)=>{
                    log::debug!("Received MicAudioChunk with {} bytes", data.len());
                },
                Event::ServerEvent(_)=>{
                    log::info!("Received ServerEvent: {:?}", evt);
                },
            }
            Some(evt)
        }
        Ok(msg) = server.recv() => {
            match msg {
                Event::ServerEvent(ServerEvent::AudioChunk { .. })=>{
                    log::info!("Received AudioChunk");
                }
                Event::ServerEvent(ServerEvent::HelloChunk { .. })=>{
                    log::info!("Received HelloChunk");
                }
                Event::ServerEvent(ServerEvent::BGChunk { .. })=>{
                    log::info!("Received BGChunk");
                }
                _=> {
                    log::info!("Received message: {:?}", msg);
                }
            }
            Some(msg)
        }
        else => {
            log::info!("No events");
            None
        }
    }
}
// 每次 mic 采集完成后, 判断是否超过 30s, 如果超过, 则更新need_compute为 true
// 当server 发送本轮 StartAudio 时, 如果 need_compute 为 true, reset metrics
// 在 server 发送 AudioChunk 过程中, 如果 need_compute为 true, metrics 统计数据长度
//  - 如果当前speed < 1, 直接播放音频片段
//  - 如果当前speed > 1, 缓存音频片段到 audio_buffer 里
// 当server 发送 EndAudio 时, 如果 need_compute为 true, metrics 计算 speed, 重置 need_compute
//  - 如果 speed > 1, 则播放缓存的 audio_buffer
struct DownloadMetrics {
    start_time: std::time::Instant,
    data_size: usize,
    timeout_sec: u64,
}

impl DownloadMetrics {
    fn new() -> Self {
        Self {
            start_time: std::time::Instant::now() - std::time::Duration::from_secs(300),
            data_size: 0,
            timeout_sec: 30,
        }
    }

    fn is_timeout(&self) -> bool {
        self.start_time.elapsed().as_secs() > self.timeout_sec
    }
    fn reset(&mut self) {
        self.start_time = std::time::Instant::now();
        self.data_size = 0;
    }

    fn add_data(&mut self, size: usize) {
        self.data_size += size;
    }

    fn elapsed(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }

    // 如果 speed > 1.0, 表示从接收数据开始到现在的耗时, 大于音频的长度
    // 如果 speed < 1.0, 表示从接收数据开始到现在的耗时, 小于音频的长度
    fn speed(&self) -> f64 {
        // 先取出自从上次重置计时(即开始接收server audio时), 到本次调用时的秒数
        // data_size / 32k 代表本次音频的时间长度
        self.elapsed().as_secs_f64() / ((self.data_size as f64) / 32000.0)
    }
}

// TODO: 按键打断
// TODO: 超时不监听
pub async fn main_work<'d>(
    mut server: Server,
    player_tx: audio::PlayerTx,
    mut evt_rx: mpsc::Receiver<Event>,
    backgroud_buffer: Option<&'d [u8]>,
) -> anyhow::Result<()> {
    #[derive(PartialEq, Eq)]
    enum State {
        Listening,
        Recording,
        Wait,
        Speaking,
        Idle,
    }
    // 创建新的 gui 实例, 并刷新背景图
    let mut gui = crate::ui::UI::new(backgroud_buffer)?;
    gui.state = "Idle".to_string();
    gui.display_flush().unwrap();

    let mut new_gui_bg = vec![];

    let mut state = State::Idle;

    let mut submit_audio = 0.0;

    let mut audio_buffer = Vec::with_capacity(8192);

    let mut metrics = DownloadMetrics::new();
    let mut need_compute = true;
    let mut speed = 0.8;
    //循环监听 evt_rx 和 server
    while let Some(evt) = select_evt(&mut evt_rx, &mut server).await {
        match evt {
            // 如果是 gaia 或 k0 事件,
            Event::Event(Event::GAIA | Event::K0) => {
                log::info!("Received event: gaia");
                // gui.state = "gaia".to_string();
                // gui.display_flush().unwrap();
                // 如果状态是Listening
                if state == State::Listening {
                    state = State::Idle; //切换至Idle
                    gui.state = "Idle".to_string();
                    gui.display_flush().unwrap(); //刷新 gui
                } else {
                    // 创建 oneshot 的channel
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    // 将这个 tx channel, 使用 AudioData::Hello封装, 然后通过player_tx发送出去
                    player_tx
                        .send(AudioData::Hello(tx))
                        .map_err(|e| anyhow::anyhow!("Error sending hello: {e:?}"))?;
                    log::info!("Waiting for hello response");
                    // 接收到 tx 的代码, 需要回复点什么, 当作 ack
                    let _ = rx.await;
                    log::info!("Hello response received");
                    // 更新为 Listening 状态
                    state = State::Listening;
                    gui.state = "Listening...".to_string();
                    gui.display_flush().unwrap();
                }
            }
            // 如果是按键松开事件
            Event::Event(Event::K0_) => {
                // 如果是Idle 或 Listening 状态
                if state == State::Idle || state == State::Listening {
                    log::info!("Received event: K0_");
                    state = State::Recording; //更新为Recording状态
                    gui.state = "Recording...".to_string();
                    gui.text = String::new();
                    gui.display_flush().unwrap(); //刷新 gui
                } else {
                    log::warn!("Received K0_ while not idle");
                }
            }
            // 这几个 Event 类型暂不作任何处理
            Event::Event(Event::RESET | Event::K2) => {}
            Event::Event(Event::YES | Event::K1) => {}
            Event::Event(Event::NO) => {}
            Event::Event(evt) => {
                log::info!("Received event: {:?}", evt);
            }
            // 如果是 mic 的音频数据段
            Event::MicAudioChunk(data) => {
                // 确保是 Listening 或 Recording 状态
                if state == State::Listening || state == State::Recording {
                    // 这里的比例 32000.0 是基于音频采样率和格式计算得出的：
                    // 采样率为 16000Hz（每秒16000个样本）
                    // 每个样本占用2字节（16位音频）
                    // 因此，每秒的音频数据大小为：16000 × 2 = 32000字节
                    // 所以， data.len() as f32 / 32000.0 计算的是当前音频数据块代表的时间长度（秒）。
                    submit_audio += data.len() as f32 / 32000.0;
                    // 将收到的数据填充到 audio_buffer 中
                    audio_buffer.extend_from_slice(&data);
                    // 如果 buffer 长度足够
                    if audio_buffer.len() >= 8192 {
                        // 将数据封装进 Message::binary 后发送给 sever
                        server
                            .send(Message::binary(bytes::Bytes::from(audio_buffer)))
                            .await?;
                        // 然后清空 buffer
                        audio_buffer = Vec::with_capacity(8192);
                    }
                } else {
                    log::debug!("Received MicAudioChunk while not listening");
                }
            }
            // 如果是 mic 音频结束事件
            Event::MicAudioEnd => {
                // 确保是 Listening 或 Recording 状态, 且submit_audio 已经累计超过 1.0s
                // 则认为是一个有效的 mic 结束事件
                if (state == State::Listening || state == State::Recording) && submit_audio > 1.0 {
                    // 如果 buffer 不为空, 发送给 server
                    if !audio_buffer.is_empty() {
                        server
                            .send(Message::binary(bytes::Bytes::from(audio_buffer)))
                            .await?;
                        audio_buffer = Vec::with_capacity(8192);
                    }
                    // 如果当前状态是 Listening, 则向 srever 发送End:Normal
                    if state == State::Listening {
                        server.send(Message::text("End:Normal")).await?;
                    } else {
                        // 如果是其他状态, 则向 server 发送 End:Recording
                        server.send(Message::text("End:Recording")).await?;
                    }
                    // 记录是否超时30s
                    // 如果本次 mic 采集已经超过 30s, 则认为是超时
                    // 这将导致 audio 播放时重新计算 speed
                    need_compute = metrics.is_timeout();
                }
                submit_audio = 0.0;
            }
            // 收到 server 的 ASR, 刷新到 gui
            Event::ServerEvent(ServerEvent::ASR { text }) => {
                log::info!("Received ASR: {:?}", text);
                gui.state = "ASR".to_string();
                gui.text = text.trim().to_string();
                gui.display_flush().unwrap();
            }
            // 收到 server 的 Action(预留给语音指令?), 刷新到 gui
            Event::ServerEvent(ServerEvent::Action { action }) => {
                log::info!("Received action");
                gui.state = format!("Action: {}", action);
                gui.display_flush().unwrap();
            }
            // 收到 server 的 StartAudio, 刷新到 gui
            Event::ServerEvent(ServerEvent::StartAudio { text }) => {
                // 第一次运行一定是 true
                if need_compute {
                    //重置计时和数据长度
                    metrics.reset();
                }
                log::info!("Received audio start: {:?}", text);
                state = State::Speaking; //更新为 Speaking
                gui.state = format!("[{:.2}x]|Speaking...", speed);
                gui.text = text.trim().to_string();
                gui.display_flush().unwrap();
                // 通过 player_tx 发送 Start 事件
                player_tx
                    .send(AudioData::Start)
                    .map_err(|e| anyhow::anyhow!("Error sending start: {e:?}"))?;
            }
            // 收到 server 的 AudioChunk, 刷新到 gui
            Event::ServerEvent(ServerEvent::AudioChunk { data }) => {
                log::info!("Received audio chunk");
                // 确保是 Speaking 状态, 否则进入下一轮
                if state != State::Speaking {
                    log::warn!("Received audio chunk while not speaking");
                    continue;
                }
                // 记录本次数据的长度到metrics
                if need_compute {
                    metrics.add_data(data.len());
                }
                // 如果当前 speed 小于1.0, 则直接发送音频数据(给扬声器播放)
                // 否则将数据缓存到 audio_buffer 里
                if speed < 1.0 {
                    // 如果失败, 要刷新 gui 提示
                    if let Err(e) = player_tx.send(AudioData::Chunk(data)) {
                        log::error!("Error sending audio chunk: {:?}", e);
                        gui.state = "Error on audio chunk".to_string();
                        gui.display_flush().unwrap();
                    }
                } else {
                    audio_buffer.extend_from_slice(&data);
                }
            }
            // 收到 server 的 EndAudio, 刷新到 gui
            Event::ServerEvent(ServerEvent::EndAudio) => {
                log::info!("Received audio end");

                if need_compute {
                    // 本次 server 传过来的音频数据已经结束, 计算 speed
                    // 即从 server 传输的 audio 开始到此刻的耗时, 和本次数据长度(秒数)
                    // 作一个比值, 即是所谓的 speed
                    speed = metrics.speed();
                    // 计算完成后, 更新 need_compute 为 false
                    // 直至下一次mic采集超过 30s
                    need_compute = false;
                }

                log::info!("Audio speed: {:.2}x", speed);
                // 如果当前 speed > 1.0, 通过之前缓存的 audio_buffer 播放
                if speed > 1.0 && audio_buffer.len() > 0 {
                    if let Err(e) = player_tx.send(AudioData::Chunk(audio_buffer)) {
                        log::error!("Error sending audio chunk: {:?}", e);
                        gui.state = "Error on audio chunk".to_string();
                        gui.display_flush().unwrap();
                    }
                    // 清空 audio_buffer
                    audio_buffer = Vec::with_capacity(8192);
                }
                // 如法炮制, 发送 End 事件, 等待扬声器线程回复一个 ack(播放完成)
                let (tx, rx) = tokio::sync::oneshot::channel();
                if let Err(e) = player_tx.send(AudioData::End(tx)) {
                    log::error!("Error sending audio chunk: {:?}", e);
                    gui.state = "Error on audio chunk".to_string();
                    gui.display_flush().unwrap();
                }
                //等待 ack
                let _ = rx.await;
                gui.display_flush().unwrap();
            }
            // 收到 server 的 EndResponse, 刷新到 gui
            Event::ServerEvent(ServerEvent::EndResponse) => {
                log::info!("Received request end");
                state = State::Listening;
                gui.state = "Listening...".to_string();
                gui.display_flush().unwrap();
            }
            // 以下是 hello 相关的分支
            Event::ServerEvent(ServerEvent::HelloStart) => {
                if let Err(_) = player_tx.send(AudioData::SetHelloStart) {
                    log::error!("Error sending hello start");
                    gui.state = "Error on hello start".to_string();
                    gui.display_flush().unwrap();
                }
            }
            Event::ServerEvent(ServerEvent::HelloChunk { data }) => {
                log::info!("Received hello chunk");
                if let Err(_) = player_tx.send(AudioData::SetHelloChunk(data.to_vec())) {
                    log::error!("Error sending hello chunk");
                    gui.state = "Error on hello chunk".to_string();
                    gui.display_flush().unwrap();
                }
            }
            Event::ServerEvent(ServerEvent::HelloEnd) => {
                log::info!("Received hello end");
                if let Err(_) = player_tx.send(AudioData::SetHelloEnd) {
                    log::error!("Error sending hello end");
                    gui.state = "Error on hello end".to_string();
                    gui.display_flush().unwrap();
                } else {
                    gui.state = "Hello set".to_string();
                    gui.display_flush().unwrap();
                }
            }
            // 以下是背景图片相关的分支
            Event::ServerEvent(ServerEvent::BGStart) => {
                new_gui_bg = vec![];
            }
            Event::ServerEvent(ServerEvent::BGChunk { data }) => {
                log::info!("Received background chunk");
                new_gui_bg.extend(data);
            }
            Event::ServerEvent(ServerEvent::BGEnd) => {
                log::info!("Received background end");
                if !new_gui_bg.is_empty() {
                    let gui_ = crate::ui::UI::new(Some(&new_gui_bg));
                    new_gui_bg.clear();
                    match gui_ {
                        Ok(new_gui) => {
                            gui = new_gui;
                            gui.state = "Background data loaded".to_string();
                            gui.display_flush().unwrap();
                        }
                        Err(e) => {
                            log::error!("Error creating GUI from background data: {:?}", e);
                            gui.state = "Error on background data".to_string();
                            gui.display_flush().unwrap();
                        }
                    }
                } else {
                    log::warn!("Received empty background data");
                }
            }
            // 预留给video
            Event::ServerEvent(ServerEvent::StartVideo | ServerEvent::EndVideo) => {}
        }
    }

    log::info!("Main work done");

    Ok(())
}
