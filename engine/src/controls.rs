use std::collections::VecDeque;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::Error;
use rodio::cpal::traits::HostTrait;
use rodio::cpal::{self, DeviceId};
use rodio::{Decoder, DeviceSinkBuilder, DeviceTrait, MixerDeviceSink, Player, Source};
use serde::Serialize;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::{chunking, tts};

/// How long the playhead may sit still while it should be moving before the
/// output counts as dead and gets reopened.
const STALL_AFTER: Duration = Duration::from_secs(3);
/// Played again from before the point where the output died, so a dropout
/// does not cost him the middle of a sentence.
const REWIND_ON_RECOVERY: Duration = Duration::from_secs(2);
/// A chunk that has not arrived by then is given up on so the rest can play.
const FETCH_DEADLINE: Duration = Duration::from_secs(180);
/// "Speaking" means the playhead moved within this window.
const MOVING_WITHIN: Duration = Duration::from_secs(1);
const WATCH_EVERY: Duration = Duration::from_millis(250);
/// Every this many watchdog ticks, check whether the default output changed.
const DEVICE_CHECK_TICKS: u32 = 4;

/// One chunk of audio on its way back from OpenAI or off the disk. `generation`
/// is stamped when the request was made, so a Stop can throw away audio that is
/// still in flight instead of letting it start talking again half a second later.
#[derive(Debug)]
pub struct Job {
    pub generation: u64,
    pub fetch: JoinHandle<Result<Vec<u8>, Error>>,
}

/// What the player is doing right now. Flattened into every IPC reply.
#[derive(Debug, Serialize)]
pub struct Status {
    /// Sound is coming out: the playhead moved in the last second.
    pub speaking: bool,
    pub paused: bool,
    /// Chunks not finished yet, whether loaded or still loading.
    pub queued: usize,
    /// Of those, chunks still being fetched or read.
    pub loading: usize,
    /// Should be playing and is not. The watchdog reopens the output after a few seconds.
    pub stalled: bool,
    /// The output device the audio goes to.
    pub device: String,
}

#[derive(Debug, Clone, Copy)]
struct QueueState {
    generation: u64,
    loading: usize,
}

/// One open output device with a player on it. Replaced whole on Stop, on a
/// stall and when the default device changes: rodio acts on stop and clear
/// only when the device pulls audio, so on a dead device they do nothing, and
/// cpal pauses a stream for good when its device drops out.
struct Output {
    player: Player,
    _sink: MixerDeviceSink,
    device: Option<DeviceId>,
    device_name: String,
}

impl Output {
    fn open() -> Result<Self, Error> {
        let default = cpal::default_host().default_output_device();
        let mut sink = DeviceSinkBuilder::open_default_sink()?;
        sink.log_on_drop(false);

        Ok(Self {
            player: Player::connect_new(sink.mixer()),
            _sink: sink,
            device: default.as_ref().and_then(|d| d.id().ok()),
            device_name: default
                .and_then(|d| d.description().ok())
                .map(|d| d.name().to_string())
                .unwrap_or_default(),
        })
    }
}

struct Playback {
    output: Output,
    /// Audio of every chunk handed to the player and not finished, oldest
    /// first, so the one playing is at the front. Kept so a dead output can be
    /// reopened without losing the queue.
    chunks: VecDeque<Arc<[u8]>>,
    /// Chunks left and playhead position at the last check, and when they last changed.
    mark: (usize, Duration),
    moved_at: Instant,
    /// Reopens since the playhead last moved. A chunk that will not play on a
    /// fresh output either gets dropped instead of freezing the queue.
    failed_reopens: u32,
}

impl Playback {
    /// Drops finished chunks from `chunks` and notes whether the playhead moved.
    fn sync(&mut self) {
        let left = self.output.player.len();
        while self.chunks.len() > left {
            self.chunks.pop_front();
        }

        let mark = (left, self.output.player.get_pos());
        if mark != self.mark {
            self.mark = mark;
            self.moved_at = Instant::now();
            self.failed_reopens = 0;
        }
    }

    fn should_be_moving(&self) -> bool {
        !self.chunks.is_empty() && !self.output.player.is_paused()
    }

    fn append(&mut self, audio: Arc<[u8]>) {
        match Decoder::try_from(Cursor::new(audio.clone())) {
            Ok(source) => {
                self.output.player.append(source);
                self.chunks.push_back(audio);
            }
            Err(e) => eprintln!("could not decode audio: {e}"),
        }
    }

    /// Opens the current default device and puts the unfinished chunks back on
    /// it, the first one from `resume_at`. Keeps the old output if none opens.
    fn reopen(&mut self, resume_at: Duration) -> Result<(), Error> {
        let output = Output::open()?;
        if self.output.player.is_paused() {
            output.player.pause();
        }

        for (index, audio) in self.chunks.iter().enumerate() {
            match Decoder::try_from(Cursor::new(audio.clone())) {
                Ok(source) if index == 0 => output.player.append(source.skip_duration(resume_at)),
                Ok(source) => output.player.append(source),
                Err(e) => eprintln!("could not decode audio: {e}"),
            }
        }

        self.output = output;
        self.mark = (self.output.player.len(), self.output.player.get_pos());
        self.moved_at = Instant::now();
        Ok(())
    }

    /// The playhead stopped while it should be moving: the device dropped out,
    /// or its stream died. Reopen and carry on from just before where it stopped.
    fn recover(&mut self) {
        let mut resume_at = self.output.player.get_pos().saturating_sub(REWIND_ON_RECOVERY);

        if self.failed_reopens >= 2 {
            eprintln!("a chunk would not play on a fresh output either, skipped it");
            self.chunks.pop_front();
            self.failed_reopens = 0;
            resume_at = Duration::ZERO;
        }

        match self.reopen(resume_at) {
            Ok(()) => eprintln!("audio stalled, reopened on {}", self.output.device_name),
            Err(e) => {
                eprintln!("audio stalled and no output would open: {e}");
                self.moved_at = Instant::now(); // try again after another STALL_AFTER
            }
        }
        self.failed_reopens += 1;
    }

    /// Follows the Mac to a new default output, as other players do.
    fn follow_default_device(&mut self) {
        let current = cpal::default_host()
            .default_output_device()
            .and_then(|d| d.id().ok());

        if current.is_none() || current == self.output.device {
            return;
        }

        let resume_at = self.output.player.get_pos();
        match self.reopen(resume_at) {
            Ok(()) => eprintln!("default output changed, now on {}", self.output.device_name),
            Err(e) => eprintln!("default output changed but would not open: {e}"),
        }
    }

    /// Empties everything, on a fresh output so nothing old can come back.
    fn clear(&mut self) {
        self.chunks.clear();
        if let Err(e) = self.reopen(Duration::ZERO) {
            eprintln!("could not reopen the output on stop: {e}");
            // Dropping the old player marks its sounds stopped; the new one starts empty.
            self.output.player = Player::connect_new(self.output._sink.mixer());
        }
        self.output.player.play(); // a pending pause would make the next speak silent
        self.mark = (0, Duration::ZERO);
        self.moved_at = Instant::now();
        self.failed_reopens = 0;
    }
}

/// Everything the three entry points (hotkey, tray menu, socket) act on.
/// Cheap to clone: it is handles to the same shared state.
#[derive(Clone)]
pub struct Controls {
    playback: Arc<Mutex<Playback>>,
    config: Arc<Config>,
    tx: mpsc::UnboundedSender<Job>,
    queue: Arc<watch::Sender<QueueState>>,
}

fn lock(playback: &Mutex<Playback>) -> MutexGuard<'_, Playback> {
    playback.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Controls {
    /// Opens the default output device, starts the queue consumer and the
    /// watchdog that reopens the output when playback freezes.
    pub fn new(config: Arc<Config>) -> Result<Self, Error> {
        let playback = Arc::new(Mutex::new(Playback {
            output: Output::open()?,
            chunks: VecDeque::new(),
            mark: (0, Duration::ZERO),
            moved_at: Instant::now(),
            failed_reopens: 0,
        }));

        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let queue = Arc::new(watch::Sender::new(QueueState {
            generation: 0,
            loading: 0,
        }));

        // Chunks are fetched in parallel but appended in order, which is why
        // this is one task reading a channel rather than a task per chunk.
        let queue_playback = playback.clone();
        let consumer_queue = queue.clone();
        tokio::spawn(async move {
            let mut queue_rx = consumer_queue.subscribe();

            while let Some(mut job) = rx.recv().await {
                let generation = job.generation;

                // Stop stops the wait too. Dropping the handle does not cancel
                // the fetch, so audio already paid for still gets saved.
                let fetched = tokio::select! {
                    result = &mut job.fetch => match result {
                        Ok(Ok(bytes)) => Some(bytes),
                        Ok(Err(e)) => {
                            eprintln!("tts request failed: {e}");
                            None
                        }
                        Err(e) => {
                            eprintln!("tts task failed: {e}");
                            None
                        }
                    },
                    _ = async {
                        let _ = queue_rx.wait_for(|q| q.generation != generation).await;
                    } => None,
                    _ = tokio::time::sleep(FETCH_DEADLINE) => {
                        eprintln!("a chunk took over {}s to arrive, skipped it", FETCH_DEADLINE.as_secs());
                        None
                    }
                };

                let mut playback = lock(&queue_playback);
                // Checked under the lock Stop clears under, so a chunk cannot
                // land in the player after the Stop that should have dropped it.
                if consumer_queue.borrow().generation == generation
                    && let Some(bytes) = fetched
                {
                    playback.append(bytes.into());
                }
                consumer_queue.send_if_modified(|q| {
                    let current = q.generation == generation && q.loading > 0;
                    if current {
                        q.loading -= 1;
                    }
                    current
                });
            }
        });

        let watch_playback = playback.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(WATCH_EVERY);
            let mut ticks: u32 = 0;

            loop {
                interval.tick().await;
                ticks = ticks.wrapping_add(1);

                let mut playback = lock(&watch_playback);
                playback.sync();

                if playback.should_be_moving() && playback.moved_at.elapsed() > STALL_AFTER {
                    playback.recover();
                } else if ticks % DEVICE_CHECK_TICKS == 0 {
                    playback.follow_default_device();
                }
            }
        });

        Ok(Self {
            playback,
            config,
            tx,
            queue,
        })
    }

    /// Counts `count` chunks as loading under the current generation and
    /// returns that generation, in one step so a Stop cannot land in between.
    fn reserve(&self, count: usize) -> u64 {
        let mut generation = 0;
        self.queue.send_modify(|q| {
            q.loading += count;
            generation = q.generation;
        });
        generation
    }

    /// Splits `text` into chunks, starts a fetch for each and queues them.
    /// Returns immediately with the number of chunks; the first one usually
    /// starts playing about a second later. With `save_dir`, every chunk is
    /// also written there so it can be replayed without paying for it again.
    pub fn speak(
        &self,
        text: &str,
        voice: Option<String>,
        instructions: Option<String>,
        save_dir: Option<PathBuf>,
    ) -> Result<usize, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(0);
        }

        let request = tts::SpeechRequest {
            voice: voice.unwrap_or_else(|| self.config.voice.clone()),
            instructions: instructions.unwrap_or_else(|| self.config.instructions.clone()),
        };

        let chunks = chunking::chunk_text(text)?;
        let count = chunks.len();

        if let Some(dir) = &save_dir {
            std::fs::create_dir_all(dir)?;
        }

        // New text queues behind whatever is already playing, so it keeps the
        // current generation. Only stop() invalidates.
        let generation = self.reserve(count);

        for (index, chunk) in chunks.into_iter().enumerate() {
            let text = chunk.to_string();
            let config = self.config.clone();
            let request = request.clone();
            let save_to = save_dir.as_ref().map(|dir| dir.join(format!("{:03}", index + 1)));

            let fetch = tokio::spawn(async move {
                let bytes = tts::get_audio_bytes(text.clone(), config, request).await?;
                // Saved even if a Stop lands meanwhile: the audio is paid for either way.
                if let Some(stem) = save_to {
                    std::fs::write(stem.with_extension("mp3"), &bytes)?;
                    std::fs::write(stem.with_extension("txt"), &text)?;
                }
                Ok(bytes)
            });
            self.tx.send(Job { generation, fetch })?;
        }

        // An earlier Pause would otherwise swallow this request silently.
        self.play();

        Ok(count)
    }

    /// Queues saved audio files behind whatever is playing. Returns how many
    /// were queued; a file that cannot be read is logged and skipped.
    pub fn play_files(&self, paths: Vec<PathBuf>) -> usize {
        let count = paths.len();
        let generation = self.reserve(count);

        for path in paths {
            let fetch = tokio::spawn(async move { Ok(tokio::fs::read(path).await?) });
            if self.tx.send(Job { generation, fetch }).is_err() {
                eprintln!("audio queue is closed");
            }
        }

        self.play();
        count
    }

    pub fn play(&self) {
        lock(&self.playback).output.player.play();
    }

    pub fn pause(&self) {
        lock(&self.playback).output.player.pause();
    }

    pub fn toggle(&self) {
        let playback = lock(&self.playback);
        if playback.output.player.is_paused() {
            playback.output.player.play();
        } else {
            playback.output.player.pause();
        }
    }

    /// Drops the chunk being spoken and moves to the next one.
    pub fn skip(&self) {
        lock(&self.playback).output.player.skip_one();
    }

    /// Drops everything: the queue, the chunk playing, and anything still
    /// downloading. Leaves the player unpaused and ready for the next request.
    pub fn stop(&self) {
        // Generation first, then the clear: the consumer checks the generation
        // under the same lock, so nothing in flight slips in after this.
        self.queue.send_modify(|q| {
            q.generation += 1;
            q.loading = 0;
        });
        lock(&self.playback).clear();
    }

    pub fn status(&self) -> Status {
        let mut playback = lock(&self.playback);
        playback.sync();

        let loading = self.queue.borrow().loading;
        let paused = playback.output.player.is_paused();
        let should_be_moving = playback.should_be_moving();
        let moving = playback.moved_at.elapsed() < MOVING_WITHIN;

        Status {
            speaking: should_be_moving && moving,
            paused,
            queued: playback.chunks.len() + loading,
            loading,
            stalled: should_be_moving && !moving,
            device: playback.output.device_name.clone(),
        }
    }
}
