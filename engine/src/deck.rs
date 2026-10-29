//! The queue as one list with a cursor: everything before the cursor has been
//! played, the entry at it is playing, everything after it is up next. Restart,
//! seek, previous, jump and remove are all just moves of the cursor or the
//! position inside the current entry, which is why there is one list and not
//! three.
//!
//! No audio device in here, so all of it is unit tested.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

/// Every chunk is decoded to mono at this rate, whatever it arrived as.
pub const RATE: u32 = 24_000;
/// Previous restarts the chunk when it is further in than this, as music players do.
const RESTART_WITHIN: Duration = Duration::from_secs(3);
/// Played entries kept for going back. Their mp3 bytes are about 1 MB a minute.
const HISTORY_LIMIT: usize = 300;
/// Entries around the cursor that keep their decoded audio: one back, two ahead.
const KEEP_BEHIND: usize = 1;
const KEEP_AHEAD: usize = 2;

pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Loading,
    Ready,
    Failed,
}

pub struct Entry {
    pub id: u64,
    /// Entries from the same speak share this, so the app can show them as one transcript.
    pub reading: u64,
    pub title: Arc<str>,
    /// 1-based position inside its reading.
    pub index: usize,
    pub text: String,
    /// The mp3 as fetched. None while it is still loading.
    pub bytes: Option<Arc<[u8]>>,
    pub failed: bool,
    /// Decoded audio, kept only near the cursor.
    pub pcm: Option<Arc<[f32]>>,
}

impl Entry {
    pub fn state(&self) -> EntryState {
        if self.failed {
            EntryState::Failed
        } else if self.bytes.is_some() {
            EntryState::Ready
        } else {
            EntryState::Loading
        }
    }

    pub fn duration(&self) -> Option<Duration> {
        self.pcm.as_ref().map(|pcm| frames_to_duration(pcm.len()))
    }
}

pub struct Deck {
    entries: Vec<Entry>,
    /// Index of the playing entry; equal to `entries.len()` when everything has played.
    cursor: usize,
    /// Frames into the current entry.
    pos: usize,
    pub paused: bool,
    speed: f32,
    volume: f32,
    /// Bumped on every jump in the audio, so the stretcher drops what it buffered.
    epoch: u64,
    /// Frames the output has pulled, silence included. Stands still only when
    /// the device stopped asking, which is what the stall watchdog looks for.
    pub rendered: u64,
    /// Bumped whenever the list itself changes, so the app refetches it only then.
    revision: u64,
    next_id: u64,
    next_reading: u64,
}

pub fn frames_to_duration(frames: usize) -> Duration {
    Duration::from_secs_f64(frames as f64 / RATE as f64)
}

fn duration_to_frames(duration: Duration) -> usize {
    (duration.as_secs_f64() * RATE as f64) as usize
}

/// The first few words, for a reading that came without a title.
fn title_from(text: &str) -> String {
    text.split_whitespace().take(8).collect::<Vec<_>>().join(" ")
}

impl Deck {
    pub fn new(speed: f32, volume: f32) -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            pos: 0,
            paused: false,
            speed: speed.clamp(MIN_SPEED, MAX_SPEED),
            volume: volume.clamp(0.0, 1.0),
            epoch: 0,
            rendered: 0,
            revision: 0,
            next_id: 1,
            next_reading: 1,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub fn position(&self) -> Duration {
        frames_to_duration(self.pos)
    }

    /// Something is ready to play at the cursor, so sound should be coming out
    /// unless paused.
    pub fn has_audio(&self) -> bool {
        self.current().is_some_and(|e| e.pcm.is_some())
    }

    pub fn loading(&self) -> usize {
        self.entries[self.cursor.min(self.entries.len())..]
            .iter()
            .filter(|e| e.state() == EntryState::Loading)
            .count()
    }

    /// The current entry and everything after it.
    pub fn queued(&self) -> usize {
        self.entries.len().saturating_sub(self.cursor)
    }

    fn changed(&mut self) {
        self.revision += 1;
    }

    fn jumped(&mut self) {
        self.epoch += 1;
        self.changed();
    }

    /// Adds one reading as loading entries at the end and returns their ids in order.
    pub fn add_reading(&mut self, title: &str, texts: Vec<String>) -> Vec<u64> {
        let reading = self.next_reading;
        self.next_reading += 1;

        let title: Arc<str> = match title.trim() {
            "" => title_from(texts.first().map(String::as_str).unwrap_or_default()).into(),
            t => t.into(),
        };

        let ids = texts
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                let id = self.next_id;
                self.next_id += 1;
                self.entries.push(Entry {
                    id,
                    reading,
                    title: title.clone(),
                    index: index + 1,
                    text,
                    bytes: None,
                    failed: false,
                    pcm: None,
                });
                id
            })
            .collect();

        self.changed();
        ids
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    /// Lands fetched audio, or marks the fetch failed. False when the entry
    /// was removed or stopped meanwhile, so the audio has nowhere to go.
    pub fn fill(&mut self, id: u64, bytes: Option<Arc<[u8]>>) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };

        let entry = &mut self.entries[index];
        match bytes {
            Some(bytes) => entry.bytes = Some(bytes),
            None => entry.failed = true,
        }
        self.changed();
        true
    }

    /// Entries near the cursor whose audio still needs decoding.
    pub fn needs_pcm(&self) -> Vec<(u64, Arc<[u8]>)> {
        let start = self.cursor.saturating_sub(KEEP_BEHIND);
        let end = (self.cursor + KEEP_AHEAD + 1).min(self.entries.len());

        self.entries
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .filter(|e| e.pcm.is_none() && !e.failed)
            .filter_map(|e| Some((e.id, e.bytes.clone()?)))
            .collect()
    }

    pub fn set_pcm(&mut self, id: u64, pcm: Option<Arc<[f32]>>) {
        let Some(index) = self.index_of(id) else {
            return;
        };

        match pcm {
            Some(pcm) => self.entries[index].pcm = Some(pcm),
            // Audio that will not decode is as good as a failed fetch.
            None => {
                self.entries[index].failed = true;
                self.changed();
            }
        }
    }

    /// Frees decoded audio away from the cursor; the mp3 stays for going back.
    pub fn trim_pcm(&mut self) {
        let start = self.cursor.saturating_sub(KEEP_BEHIND);
        let end = self.cursor + KEEP_AHEAD;

        for (index, entry) in self.entries.iter_mut().enumerate() {
            if index < start || index > end {
                entry.pcm = None;
            }
        }
    }

    fn advance(&mut self) {
        if self.cursor < self.entries.len() {
            self.cursor += 1;
        }
        self.pos = 0;

        if self.cursor > HISTORY_LIMIT {
            let excess = self.cursor - HISTORY_LIMIT;
            self.entries.drain(..excess);
            self.cursor -= excess;
        }
        self.changed();
    }

    /// Fills `out` with the next frames, moving on to the next entry when one
    /// ends. Stops early when the next entry is still loading or decoding, and
    /// returns how many frames it wrote.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let mut written = 0;

        while written < out.len() {
            let Some(entry) = self.entries.get(self.cursor) else {
                break;
            };

            if entry.failed {
                self.advance();
                continue;
            }

            let Some(pcm) = entry.pcm.clone() else {
                break;
            };

            let take = (pcm.len() - self.pos.min(pcm.len())).min(out.len() - written);
            out[written..written + take].copy_from_slice(&pcm[self.pos..self.pos + take]);
            written += take;
            self.pos += take;

            if self.pos >= pcm.len() {
                self.advance();
            }
        }

        written
    }

    pub fn restart(&mut self) {
        self.pos = 0;
        self.jumped();
    }

    /// Moves within the current entry. Past the end goes to the next entry;
    /// before the start stops at the start.
    pub fn seek_by(&mut self, seconds: f64) {
        let target = self.pos as f64 / RATE as f64 + seconds;
        self.seek_to(Duration::from_secs_f64(target.max(0.0)));
    }

    pub fn seek_to(&mut self, at: Duration) {
        let Some(len) = self.current().and_then(|e| e.pcm.as_ref()).map(|p| p.len()) else {
            return;
        };

        let frames = duration_to_frames(at);
        if frames >= len {
            self.advance();
        } else {
            self.pos = frames;
        }
        self.jumped();
    }

    /// Back to the start of this entry, or to the one before when already near the start.
    pub fn previous(&mut self) {
        if self.cursor > 0 && (self.pos < duration_to_frames(RESTART_WITHIN) || self.cursor >= self.entries.len()) {
            self.cursor -= 1;
        }
        self.pos = 0;
        self.jumped();
    }

    pub fn next(&mut self) {
        self.advance();
        self.jumped();
    }

    /// Plays `id` from its start. Everything before it counts as played.
    pub fn jump(&mut self, id: u64) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };

        self.cursor = index;
        self.pos = 0;
        self.jumped();
        true
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };

        self.entries.remove(index);
        if index < self.cursor {
            self.cursor -= 1;
        } else if index == self.cursor {
            self.pos = 0;
            self.epoch += 1;
        }
        self.changed();
        true
    }

    /// Drops everything up next. The entry playing moves to played, since he heard part of it.
    pub fn stop(&mut self) {
        let keep = (self.cursor + 1).min(self.entries.len());
        self.entries.truncate(keep);
        self.cursor = self.entries.len();
        self.pos = 0;
        self.paused = false;
        self.jumped();
    }

    pub fn clear_played(&mut self) {
        let played = self.cursor.min(self.entries.len());
        self.entries.drain(..played);
        self.cursor -= played;
        self.changed();
    }

    /// Steps back after the output died, so a dropout does not cost the middle of a sentence.
    pub fn rewind(&mut self, by: Duration) {
        self.pos = self.pos.saturating_sub(duration_to_frames(by));
        self.epoch += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deck with `n` ready entries of one second each, every sample equal to its entry number.
    fn deck_with(n: usize) -> (Deck, Vec<u64>) {
        let mut deck = Deck::new(1.0, 1.0);
        let ids = deck.add_reading("t", (1..=n).map(|i| format!("chunk {i}")).collect());
        for (i, id) in ids.iter().enumerate() {
            deck.fill(*id, Some(Arc::from(vec![0u8; 4])));
            deck.set_pcm(*id, Some(Arc::from(vec![(i + 1) as f32; RATE as usize])));
        }
        (deck, ids)
    }

    fn current_id(deck: &Deck) -> Option<u64> {
        deck.current().map(|e| e.id)
    }

    #[test]
    fn reads_across_entries_and_ends() {
        let (mut deck, ids) = deck_with(2);
        let mut out = vec![0.0; RATE as usize + 10];

        assert_eq!(deck.read(&mut out), out.len());
        assert_eq!(out[RATE as usize - 1], 1.0);
        assert_eq!(out[RATE as usize], 2.0);
        assert_eq!(current_id(&deck), Some(ids[1]));

        let mut rest = vec![0.0; RATE as usize];
        assert_eq!(deck.read(&mut rest), RATE as usize - 10);
        assert_eq!(deck.cursor(), 2);
        assert!(deck.current().is_none());
    }

    #[test]
    fn waits_for_an_entry_still_loading() {
        let mut deck = Deck::new(1.0, 1.0);
        let ids = deck.add_reading("", vec!["a".into(), "b".into()]);
        deck.fill(ids[0], Some(Arc::from(vec![0u8])));
        deck.set_pcm(ids[0], Some(Arc::from(vec![1.0; 100])));

        let mut out = vec![0.0; 300];
        assert_eq!(deck.read(&mut out), 100);
        assert_eq!(current_id(&deck), Some(ids[1]));
        assert_eq!(deck.loading(), 1);
        assert_eq!(deck.read(&mut out), 0);
    }

    #[test]
    fn failed_entries_are_skipped() {
        let (mut deck, ids) = deck_with(3);
        deck.fill(ids[1], None);
        deck.next();
        let mut out = vec![0.0; 10];
        deck.read(&mut out);
        assert_eq!(current_id(&deck), Some(ids[2]));
        assert_eq!(out[0], 3.0);
    }

    #[test]
    fn previous_restarts_late_in_a_chunk_and_steps_back_early() {
        let (mut deck, ids) = deck_with(3);
        deck.next();
        deck.seek_to(Duration::from_millis(500));
        deck.previous();
        assert_eq!(current_id(&deck), Some(ids[0]));

        let mut long = Deck::new(1.0, 1.0);
        let lid = long.add_reading("", vec!["a".into(), "b".into()]);
        for id in &lid {
            long.fill(*id, Some(Arc::from(vec![0u8])));
            long.set_pcm(*id, Some(Arc::from(vec![0.0; RATE as usize * 10])));
        }
        long.next();
        long.seek_to(Duration::from_secs(5));
        long.previous();
        assert_eq!(current_id(&long), Some(lid[1]));
        assert_eq!(long.position(), Duration::ZERO);
    }

    #[test]
    fn previous_after_the_end_goes_to_the_last_entry() {
        let (mut deck, ids) = deck_with(2);
        deck.next();
        deck.next();
        assert!(deck.current().is_none());
        deck.previous();
        assert_eq!(current_id(&deck), Some(ids[1]));
    }

    #[test]
    fn seek_clamps_at_start_and_moves_on_past_the_end() {
        let (mut deck, ids) = deck_with(2);
        deck.seek_by(-5.0);
        assert_eq!(deck.position(), Duration::ZERO);
        deck.seek_by(0.25);
        assert_eq!(deck.position(), Duration::from_millis(250));
        deck.seek_by(10.0);
        assert_eq!(current_id(&deck), Some(ids[1]));
        assert_eq!(deck.position(), Duration::ZERO);
    }

    #[test]
    fn jump_moves_the_cursor_both_ways() {
        let (mut deck, ids) = deck_with(4);
        assert!(deck.jump(ids[2]));
        assert_eq!(deck.cursor(), 2);
        assert!(deck.jump(ids[0]));
        assert_eq!(deck.cursor(), 0);
        assert!(!deck.jump(999));
    }

    #[test]
    fn remove_keeps_the_cursor_on_the_same_entry() {
        let (mut deck, ids) = deck_with(4);
        deck.jump(ids[2]);
        deck.remove(ids[0]);
        assert_eq!(current_id(&deck), Some(ids[2]));
        deck.remove(ids[3]);
        assert_eq!(current_id(&deck), Some(ids[2]));
        deck.remove(ids[2]);
        assert!(deck.current().is_none());
        assert_eq!(deck.entries().len(), 1);
    }

    #[test]
    fn removing_the_current_entry_plays_the_next() {
        let (mut deck, ids) = deck_with(3);
        deck.seek_to(Duration::from_millis(300));
        deck.remove(ids[0]);
        assert_eq!(current_id(&deck), Some(ids[1]));
        assert_eq!(deck.position(), Duration::ZERO);
    }

    #[test]
    fn stop_keeps_history_and_drops_what_is_up_next() {
        let (mut deck, ids) = deck_with(4);
        deck.jump(ids[1]);
        deck.paused = true;
        deck.stop();
        assert_eq!(deck.entries().len(), 2);
        assert!(deck.current().is_none());
        assert!(!deck.paused);

        let new = deck.add_reading("", vec!["x".into()]);
        assert_eq!(current_id(&deck), Some(new[0]));
    }

    #[test]
    fn clear_played_keeps_current_and_upcoming() {
        let (mut deck, ids) = deck_with(4);
        deck.jump(ids[2]);
        deck.clear_played();
        assert_eq!(deck.cursor(), 0);
        assert_eq!(current_id(&deck), Some(ids[2]));
        assert_eq!(deck.entries().len(), 2);
    }

    #[test]
    fn fill_after_removal_is_dropped() {
        let mut deck = Deck::new(1.0, 1.0);
        let ids = deck.add_reading("", vec!["a".into()]);
        deck.stop();
        deck.clear_played();
        assert!(!deck.fill(ids[0], Some(Arc::from(vec![0u8]))));
    }

    #[test]
    fn decoded_audio_is_kept_only_near_the_cursor() {
        let (mut deck, ids) = deck_with(6);
        deck.jump(ids[3]);
        deck.trim_pcm();
        let kept: Vec<bool> = deck.entries().iter().map(|e| e.pcm.is_some()).collect();
        assert_eq!(kept, vec![false, false, true, true, true, true]);

        deck.jump(ids[0]);
        let wanted: Vec<u64> = deck.needs_pcm().iter().map(|(id, _)| *id).collect();
        assert_eq!(wanted, vec![ids[0], ids[1]]);
    }

    #[test]
    fn history_is_capped() {
        let mut deck = Deck::new(1.0, 1.0);
        let ids = deck.add_reading("", (0..HISTORY_LIMIT + 5).map(|i| i.to_string()).collect());
        deck.jump(*ids.last().unwrap());
        deck.next();
        assert_eq!(deck.cursor(), HISTORY_LIMIT);
        assert_eq!(deck.entries().len(), HISTORY_LIMIT);
    }

    #[test]
    fn untitled_reading_takes_its_first_words() {
        let mut deck = Deck::new(1.0, 1.0);
        deck.add_reading(" ", vec!["one two three four five six seven eight nine".into()]);
        assert_eq!(&*deck.entries()[0].title, "one two three four five six seven eight");
    }

    #[test]
    fn speed_is_clamped() {
        let mut deck = Deck::new(9.0, 1.0);
        assert_eq!(deck.speed(), MAX_SPEED);
        deck.set_speed(0.1);
        assert_eq!(deck.speed(), MIN_SPEED);
    }
}
