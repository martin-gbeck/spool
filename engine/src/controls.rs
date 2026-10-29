use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use anyhow::Error;
use serde::Serialize;

use crate::audio::{self, Output, lock};
use crate::config::{self, Config};
use crate::deck::{Deck, EntryState};
use crate::{chunking, tts};

/// How long the output may stop pulling audio while it should be playing
/// before it counts as dead and gets reopened.
const STALL_AFTER: Duration = Duration::from_secs(3);
/// Played again from before the point where the output died, so a dropout
/// does not cost him the middle of a sentence.
const REWIND_ON_RECOVERY: Duration = Duration::from_secs(2);
/// A chunk that has not arrived by then is given up on so the rest can play.
const FETCH_DEADLINE: Duration = Duration::from_secs(180);
/// "Speaking" means the output pulled audio within this window.
const MOVING_WITHIN: Duration = Duration::from_secs(1);
const WATCH_EVERY: Duration = Duration::from_millis(250);
/// Every this many watchdog ticks, check whether the default output changed.
const DEVICE_CHECK_TICKS: u32 = 4;

/// What the player is doing right now. Flattened into every IPC reply, and
/// polled a few times a second by the app.
#[derive(Debug, Serialize)]
pub struct Status {
    /// Sound is coming out.
    pub speaking: bool,
    pub paused: bool,
    /// The chunk playing plus those up next, loaded or not.
    pub queued: usize,
    /// Of those, chunks still being fetched or read.
    pub loading: usize,
    /// Should be playing and is not. The watchdog reopens the output after a few seconds.
    pub stalled: bool,
    /// The output device the audio goes to.
    pub device: String,
    /// Id of the chunk playing, if any.
    pub current: Option<u64>,
    /// Seconds into the current chunk, and its length once decoded.
    pub position: f64,
    pub duration: Option<f64>,
    pub speed: f32,
    pub volume: f32,
    /// Changes whenever the list changes; the app refetches the queue only then.
    pub revision: u64,
}

/// One row of the queue as the app shows it.
#[derive(Debug, Serialize)]
pub struct EntryView {
    pub id: u64,
    pub reading: u64,
    pub title: String,
    pub index: usize,
    /// Chunks in its reading, so the app can say "3 of 12".
    pub of: usize,
    pub text: String,
    pub state: EntryState,
}

#[derive(Debug, Serialize)]
pub struct QueueView {
    pub entries: Vec<EntryView>,
    /// Index of the playing entry; equal to the length when everything has played.
    pub cursor: usize,
}

#[derive(Debug, Serialize)]
pub struct SettingsView {
    pub voice: String,
    pub model: String,
    pub instructions: String,
    pub shortcut: String,
    pub speed: f32,
    pub volume: f32,
    pub voices: Vec<&'static str>,
    pub models: Vec<&'static str>,
}

/// What a settings change may touch. Anything left out stays as it was.
#[derive(Debug, Default, serde::Deserialize, Serialize)]
pub struct SettingsChange {
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
}

struct Watch {
    /// None only for the moment between dropping a dead output and opening its replacement.
    output: Option<Output>,
    device_name: String,
    rendered: u64,
    moved_at: Instant,
}

/// Everything the hotkey and the socket act on. Cheap to clone: it is handles
/// to the same shared state.
#[derive(Clone)]
pub struct Controls {
    deck: Arc<Mutex<Deck>>,
    watch: Arc<Mutex<Watch>>,
    config: Arc<RwLock<Config>>,
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Watch {
    /// Drops the old output before opening the new one, so two outputs never
    /// pull from the deck at once and play the same audio twice.
    fn reopen(&mut self, deck: &Arc<Mutex<Deck>>) -> Result<(), Error> {
        self.output = None;
        let output = Output::open(deck.clone())?;
        self.device_name = output.device_name.clone();
        self.output = Some(output);
        self.moved_at = Instant::now();
        Ok(())
    }
}

impl Controls {
    /// Opens the default output device and starts the watchdog that decodes
    /// audio near the cursor and reopens the output when playback freezes.
    pub fn new(config: Config) -> Result<Self, Error> {
        let deck = Arc::new(Mutex::new(Deck::new(config.speed, config.volume)));
        let output = Output::open(deck.clone())?;

        let watch = Arc::new(Mutex::new(Watch {
            device_name: output.device_name.clone(),
            output: Some(output),
            rendered: 0,
            moved_at: Instant::now(),
        }));

        let controls = Self {
            deck,
            watch,
            config: Arc::new(RwLock::new(config)),
        };

        let ticking = controls.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(WATCH_EVERY);
            let mut ticks: u32 = 0;
            loop {
                interval.tick().await;
                ticks = ticks.wrapping_add(1);
                ticking.prepare();
                ticking.check_output(ticks.is_multiple_of(DEVICE_CHECK_TICKS));
            }
        });

        Ok(controls)
    }

    fn config(&self) -> Config {
        self.config.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Decodes the entries around the cursor, outside the lock so the audio
    /// thread never waits on an mp3 decoder, and frees the rest.
    fn prepare(&self) {
        let needed = lock(&self.deck).needs_pcm();
        for (id, bytes) in needed {
            let pcm = match audio::decode(bytes) {
                Ok(pcm) => Some(pcm),
                Err(e) => {
                    eprintln!("could not decode chunk {id}: {e}");
                    None
                }
            };
            lock(&self.deck).set_pcm(id, pcm);
        }
        lock(&self.deck).trim_pcm();
    }

    fn check_output(&self, check_device: bool) {
        let (rendered, should_move) = {
            let deck = lock(&self.deck);
            (deck.rendered, !deck.paused && deck.has_audio())
        };

        let mut watch = guard(&self.watch);
        if rendered != watch.rendered {
            watch.rendered = rendered;
            watch.moved_at = Instant::now();
        }

        if should_move && watch.moved_at.elapsed() > STALL_AFTER {
            lock(&self.deck).rewind(REWIND_ON_RECOVERY);
            match watch.reopen(&self.deck) {
                Ok(()) => eprintln!("audio stalled, reopened on {}", watch.device_name),
                Err(e) => {
                    eprintln!("audio stalled and no output would open: {e}");
                    watch.moved_at = Instant::now(); // try again after another STALL_AFTER
                }
            }
        } else if check_device {
            let current = audio::default_device();
            let open = watch.output.as_ref().and_then(|o| o.device.clone());
            if current.is_some() && current != open {
                match watch.reopen(&self.deck) {
                    Ok(()) => eprintln!("default output changed, now on {}", watch.device_name),
                    Err(e) => eprintln!("default output changed but would not open: {e}"),
                }
            }
        }
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
        title: Option<String>,
    ) -> Result<usize, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(0);
        }

        let config = Arc::new(self.config());
        let request = tts::SpeechRequest {
            voice: voice.unwrap_or_else(|| config.voice.clone()),
            instructions: instructions.unwrap_or_else(|| config.instructions.clone()),
        };

        let chunks: Vec<String> = chunking::chunk_text(text)?.into_iter().map(String::from).collect();
        let count = chunks.len();

        if let Some(dir) = &save_dir {
            std::fs::create_dir_all(dir)?;
        }

        let ids = {
            let mut deck = lock(&self.deck);
            // An earlier Pause would otherwise swallow this request silently.
            deck.paused = false;
            deck.add_reading(title.as_deref().unwrap_or_default(), chunks.clone())
        };

        for (index, (id, text)) in ids.into_iter().zip(chunks).enumerate() {
            let config = config.clone();
            let request = request.clone();
            let save_to = save_dir.as_ref().map(|dir| dir.join(format!("{:03}", index + 1)));
            let deck = self.deck.clone();

            tokio::spawn(async move {
                let fetched = tokio::time::timeout(
                    FETCH_DEADLINE,
                    tts::get_audio_bytes(text.clone(), config, request),
                )
                .await;

                let bytes = match fetched {
                    Ok(Ok(bytes)) => {
                        // Saved even if a Stop lands meanwhile: the audio is paid for either way.
                        if let Some(stem) = save_to
                            && let Err(e) = std::fs::write(stem.with_extension("mp3"), &bytes)
                                .and_then(|()| std::fs::write(stem.with_extension("txt"), &text))
                        {
                            eprintln!("could not save chunk {}: {e}", stem.display());
                        }
                        Some(Arc::from(bytes))
                    }
                    Ok(Err(e)) => {
                        eprintln!("tts request failed: {e}");
                        None
                    }
                    Err(_) => {
                        eprintln!("a chunk took over {}s to arrive, skipped it", FETCH_DEADLINE.as_secs());
                        None
                    }
                };
                lock(&deck).fill(id, bytes);
            });
        }

        Ok(count)
    }

    /// Queues audio files saved by an earlier speak, with the text saved beside
    /// each one so the lists show what they say. Returns how many were queued.
    pub fn play_files(&self, paths: Vec<PathBuf>, title: Option<String>) -> usize {
        let texts = paths
            .iter()
            .map(|p| std::fs::read_to_string(p.with_extension("txt")).unwrap_or_default())
            .collect();
        let title = title.unwrap_or_else(|| {
            paths
                .first()
                .and_then(|p| p.parent()?.file_name()?.to_str().map(String::from))
                .unwrap_or_default()
        });

        let ids = {
            let mut deck = lock(&self.deck);
            deck.paused = false;
            deck.add_reading(&title, texts)
        };

        let count = ids.len();
        for (id, path) in ids.into_iter().zip(paths) {
            let deck = self.deck.clone();
            tokio::spawn(async move {
                let bytes = match tokio::fs::read(&path).await {
                    Ok(bytes) => Some(Arc::from(bytes)),
                    Err(e) => {
                        eprintln!("could not read {}: {e}", path.display());
                        None
                    }
                };
                lock(&deck).fill(id, bytes);
            });
        }
        count
    }

    pub fn play(&self) {
        lock(&self.deck).paused = false;
    }

    pub fn pause(&self) {
        lock(&self.deck).paused = true;
    }

    pub fn toggle(&self) {
        let mut deck = lock(&self.deck);
        deck.paused = !deck.paused;
    }

    /// Runs `change` on the deck, then decodes what the move needs right away
    /// instead of waiting for the next watchdog tick.
    fn moved<T>(&self, change: impl FnOnce(&mut Deck) -> T) -> T {
        let result = change(&mut lock(&self.deck));
        self.prepare();
        result
    }

    /// Moves to the next chunk; the one playing goes to played.
    pub fn skip(&self) {
        self.moved(Deck::next);
    }

    pub fn previous(&self) {
        self.moved(Deck::previous);
    }

    pub fn restart(&self) {
        self.moved(Deck::restart);
    }

    pub fn seek_by(&self, seconds: f64) {
        self.moved(|deck| deck.seek_by(seconds));
    }

    pub fn seek_to(&self, seconds: f64) {
        self.moved(|deck| deck.seek_to(Duration::from_secs_f64(seconds.max(0.0))));
    }

    pub fn jump(&self, id: u64) -> bool {
        self.moved(|deck| {
            let found = deck.jump(id);
            deck.paused = false;
            found
        })
    }

    pub fn remove(&self, id: u64) -> bool {
        self.moved(|deck| deck.remove(id))
    }

    pub fn clear_played(&self) {
        lock(&self.deck).clear_played();
    }

    /// Drops everything up next and anything still downloading. Leaves the
    /// player unpaused and ready for the next request; history stays.
    pub fn stop(&self) {
        lock(&self.deck).stop();
    }

    pub fn set_speed(&self, speed: f32) -> Result<(), Error> {
        let speed = {
            let mut deck = lock(&self.deck);
            deck.set_speed(speed);
            deck.speed()
        };
        self.update_config(|c| c.speed = speed)
    }

    pub fn set_volume(&self, volume: f32) -> Result<(), Error> {
        let volume = {
            let mut deck = lock(&self.deck);
            deck.set_volume(volume);
            deck.volume()
        };
        self.update_config(|c| c.volume = volume)
    }

    pub fn change_settings(&self, change: SettingsChange) -> Result<(), Error> {
        if let Some(voice) = &change.voice {
            anyhow::ensure!(config::VOICES.contains(&voice.as_str()), "unknown voice {voice:?}");
        }
        if let Some(model) = &change.model {
            anyhow::ensure!(config::MODELS.contains(&model.as_str()), "unknown model {model:?}");
        }

        self.update_config(|c| {
            if let Some(voice) = change.voice {
                c.voice = voice;
            }
            if let Some(model) = change.model {
                c.model = model;
            }
            if let Some(instructions) = change.instructions {
                c.instructions = instructions;
            }
        })
    }

    fn update_config(&self, change: impl FnOnce(&mut Config)) -> Result<(), Error> {
        let mut config = self.config.write().unwrap_or_else(|p| p.into_inner());
        change(&mut config);
        config.save()
    }

    pub fn settings(&self) -> SettingsView {
        let config = self.config();
        SettingsView {
            voice: config.voice,
            model: config.model,
            instructions: config.instructions,
            shortcut: config.shortcut.unwrap_or_else(|| "ctrl+space".into()),
            speed: config.speed,
            volume: config.volume,
            voices: config::VOICES.to_vec(),
            models: config::MODELS.to_vec(),
        }
    }

    pub fn queue(&self) -> QueueView {
        let deck = lock(&self.deck);
        let entries = deck.entries();

        QueueView {
            entries: entries
                .iter()
                .map(|e| EntryView {
                    id: e.id,
                    reading: e.reading,
                    title: e.title.to_string(),
                    index: e.index,
                    of: entries.iter().filter(|o| o.reading == e.reading).map(|o| o.index).max().unwrap_or(e.index),
                    text: e.text.clone(),
                    state: e.state(),
                })
                .collect(),
            cursor: deck.cursor(),
        }
    }

    pub fn status(&self) -> Status {
        // Watch before deck and never both: the watchdog takes the deck while
        // holding the watch, so the other order could deadlock.
        let (moving, device) = {
            let watch = guard(&self.watch);
            (watch.moved_at.elapsed() < MOVING_WITHIN, watch.device_name.clone())
        };
        let deck = lock(&self.deck);
        let should_move = !deck.paused && deck.has_audio();

        Status {
            speaking: should_move && moving,
            paused: deck.paused,
            queued: deck.queued(),
            loading: deck.loading(),
            stalled: should_move && !moving,
            device,
            current: deck.current().map(|e| e.id),
            position: deck.position().as_secs_f64(),
            duration: deck.current().and_then(|e| e.duration()).map(|d| d.as_secs_f64()),
            speed: deck.speed(),
            volume: deck.volume(),
            revision: deck.revision(),
        }
    }
}
