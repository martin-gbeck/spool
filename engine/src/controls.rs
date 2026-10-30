use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use anyhow::Error;
use serde::Serialize;

use crate::audio::{self, Output, lock};
use crate::config::{self, Config};
use crate::deck::{Audio, Deck, EntryState, Place, Saved};
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
/// Every this many ticks the position is saved even when the list did not change.
const SAVE_POSITION_TICKS: u32 = 8;

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

/// The heading of the transcript.md that tts.py saves beside a reading's
/// audio, or the folder name when there is none.
fn reading_title(audio: &std::path::Path) -> String {
    let Some(folder) = audio.parent() else {
        return String::new();
    };
    std::fs::read_to_string(folder.join("transcript.md"))
        .ok()
        .and_then(|t| t.lines().next().map(|l| l.trim_start_matches('#').trim().to_string()))
        .filter(|t| !t.is_empty())
        .or_else(|| folder.file_name()?.to_str().map(String::from))
        .unwrap_or_default()
}

/// The queue as last saved, beside the config.
fn queue_path() -> PathBuf {
    config::config_path().with_file_name("queue.json")
}

fn load_queue() -> Option<Saved> {
    let raw = std::fs::read(queue_path()).ok()?;
    match serde_json::from_slice(&raw) {
        Ok(saved) => Some(saved),
        Err(e) => {
            eprintln!("could not read the saved queue, starting empty: {e}");
            None
        }
    }
}

/// A library folder for a reading that came without one (the hotkey, the
/// app's paste field), laid out as tts.py lays out Claude's: a dated folder
/// with transcript.md, the chunks' mp3 and txt added as they arrive.
fn new_reading(text: &str, title: Option<&str>) -> Result<PathBuf, Error> {
    let heading = match title.map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => text.split_whitespace().take(8).collect::<Vec<_>>().join(" "),
    };
    let words: Vec<String> = heading
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(8)
        .map(String::from)
        .collect();
    let slug = if words.is_empty() { "reading".to_string() } else { words.join("-") };
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M");

    let library = config::config_path().with_file_name("library");
    let mut dir = library.join(format!("{stamp}-{slug}"));
    // Two readings in the same minute with the same opening words get their own folders.
    let mut n = 2;
    while dir.exists() {
        dir = library.join(format!("{stamp}-{slug}-{n}"));
        n += 1;
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("transcript.md"), format!("# {heading}\n\n{text}\n"))?;
    Ok(dir)
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
        let deck = Arc::new(Mutex::new(match load_queue() {
            Some(saved) => Deck::restore(saved, config.speed, config.volume),
            None => Deck::new(config.speed, config.volume),
        }));
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
            let mut saved_revision = None;
            loop {
                interval.tick().await;
                ticks = ticks.wrapping_add(1);
                ticking.prepare();
                ticking.check_output(ticks.is_multiple_of(DEVICE_CHECK_TICKS));

                let revision = lock(&ticking.deck).revision();
                if saved_revision != Some(revision) || ticks.is_multiple_of(SAVE_POSITION_TICKS) {
                    match ticking.save_queue() {
                        Ok(()) => saved_revision = Some(revision),
                        Err(e) => eprintln!("could not save the queue: {e}"),
                    }
                }
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
        for (id, source) in needed {
            let pcm = match audio::decode(source) {
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
        place: Place,
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

        // Every reading is saved, the hotkey's too, so the queue can come back
        // after a restart and anything can be replayed without paying again.
        let save_dir = match save_dir {
            Some(dir) => {
                std::fs::create_dir_all(&dir)?;
                dir
            }
            None => new_reading(text, title.as_deref())?,
        };

        let ids = {
            let mut deck = lock(&self.deck);
            // An earlier Pause would otherwise swallow this request silently.
            deck.paused = false;
            deck.add_reading(title.as_deref().unwrap_or_default(), chunks.clone(), place)
        };

        for (index, (id, text)) in ids.into_iter().zip(chunks).enumerate() {
            let config = config.clone();
            let request = request.clone();
            let stem = save_dir.join(format!("{:03}", index + 1));
            let deck = self.deck.clone();

            tokio::spawn(async move {
                let fetched = tokio::time::timeout(
                    FETCH_DEADLINE,
                    tts::get_audio_bytes(text.clone(), config, request),
                )
                .await;

                let audio = match fetched {
                    // Saved even if a Stop lands meanwhile: the audio is paid for either way.
                    Ok(Ok(bytes)) => {
                        let mp3 = stem.with_extension("mp3");
                        match std::fs::write(&mp3, &bytes)
                            .and_then(|()| std::fs::write(stem.with_extension("txt"), &text))
                        {
                            Ok(()) => Some(Audio::File(mp3)),
                            Err(e) => {
                                eprintln!("could not save chunk {}: {e}", stem.display());
                                Some(Audio::Bytes(Arc::from(bytes)))
                            }
                        }
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
                lock(&deck).fill(id, audio);
            });
        }

        Ok(count)
    }

    /// Queues audio files, with the text saved beside each one so the lists
    /// show what they say. A file with no text beside it, like an mp3 from
    /// elsewhere, shows its file name instead. Returns how many were queued.
    pub fn play_files(&self, paths: Vec<PathBuf>, title: Option<String>, place: Place) -> usize {
        let texts = paths
            .iter()
            .map(|p| {
                std::fs::read_to_string(p.with_extension("txt"))
                    .ok()
                    .filter(|t| !t.trim().is_empty())
                    .or_else(|| p.file_stem()?.to_str().map(String::from))
                    .unwrap_or_default()
            })
            .collect();
        let title = title.unwrap_or_else(|| paths.first().map(|p| reading_title(p)).unwrap_or_default());

        let ids = {
            let mut deck = lock(&self.deck);
            deck.paused = false;
            deck.add_reading(&title, texts, place)
        };

        let count = ids.len();
        let mut deck = lock(&self.deck);
        for (id, path) in ids.into_iter().zip(paths) {
            let audio = if path.is_file() {
                Some(Audio::File(path))
            } else {
                eprintln!("no audio file at {}", path.display());
                None
            };
            deck.fill(id, audio);
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

    pub fn move_entry(&self, id: u64, before: Option<u64>) -> bool {
        lock(&self.deck).move_entry(id, before)
    }

    fn save_queue(&self) -> Result<(), Error> {
        let saved = lock(&self.deck).snapshot();
        let path = queue_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&saved)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
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
