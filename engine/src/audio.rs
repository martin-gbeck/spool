//! The bridge from the deck to the sound card: decoding, and the one endless
//! source every output plays. The source pulls from the deck itself, so
//! moving between chunks has no gap and every control is just a change to the
//! deck.

use std::io::Cursor;
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Error;
use rodio::cpal::traits::HostTrait;
use rodio::cpal::{self, DeviceId};
use rodio::source::UniformSourceIterator;
use rodio::{ChannelCount, Decoder, DeviceSinkBuilder, DeviceTrait, MixerDeviceSink, Player, SampleRate, Source};
use signalsmith_stretch::Stretch;

use crate::deck::{Deck, RATE};

const CHANNELS: ChannelCount = NonZero::new(1).unwrap();
const SAMPLE_RATE: SampleRate = NonZero::new(RATE).unwrap();
/// Frames per pull from the deck: 20 ms, short enough that a pause or a speed
/// change is heard at once.
const BLOCK: usize = 480;
/// Below this distance from 1x the stretcher is bypassed and the audio passes untouched.
const NORMAL_SPEED: f32 = 0.01;

/// The mp3 as mono samples at `RATE`, whatever rate and channels it came in.
pub fn decode(bytes: Arc<[u8]>) -> Result<Arc<[f32]>, Error> {
    let decoder = Decoder::try_from(Cursor::new(bytes))?;
    Ok(UniformSourceIterator::new(decoder, CHANNELS, SAMPLE_RATE).collect())
}

pub fn lock(deck: &Mutex<Deck>) -> std::sync::MutexGuard<'_, Deck> {
    deck.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Plays whatever the deck says, forever, and silence when there is nothing.
/// Speed changes go through Signalsmith Stretch, which keeps the pitch where
/// it is, as YouTube does, instead of the chipmunk effect of plain resampling.
pub struct DeckSource {
    deck: Arc<Mutex<Deck>>,
    stretch: Stretch,
    stretching: bool,
    epoch: u64,
    input: Vec<f32>,
    out: Vec<f32>,
    at: usize,
    /// Fractional input frames left over between blocks, so 1.3x averages 1.3x.
    carry: f64,
}

impl DeckSource {
    pub fn new(deck: Arc<Mutex<Deck>>) -> Self {
        let epoch = lock(&deck).epoch();
        Self {
            deck,
            stretch: Stretch::preset_default(1, RATE),
            stretching: false,
            epoch,
            input: Vec::with_capacity(BLOCK * 4),
            out: vec![0.0; BLOCK],
            at: BLOCK,
            carry: 0.0,
        }
    }

    fn render(&mut self) {
        let mut deck = lock(&self.deck);
        deck.rendered += BLOCK as u64;

        if deck.epoch() != self.epoch {
            self.epoch = deck.epoch();
            self.stretch.reset();
            self.carry = 0.0;
        }

        self.out.fill(0.0);
        if deck.paused {
            return;
        }

        let speed = deck.speed();
        let volume = deck.volume();

        if (speed - 1.0).abs() < NORMAL_SPEED {
            self.stretching = false;
            deck.read(&mut self.out);
        } else {
            if !self.stretching {
                self.stretch.reset();
                self.stretching = true;
            }

            let wanted = BLOCK as f64 * speed as f64 + self.carry;
            let frames = wanted.floor();
            self.carry = wanted - frames;

            self.input.clear();
            self.input.resize(frames as usize, 0.0);
            deck.read(&mut self.input);
            drop(deck);

            self.stretch.process(&self.input, &mut self.out);
        }

        if volume < 1.0 {
            self.out.iter_mut().for_each(|s| *s *= volume);
        }
    }
}

impl Iterator for DeckSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.at >= self.out.len() {
            self.render();
            self.at = 0;
        }
        let sample = self.out[self.at];
        self.at += 1;
        Some(sample)
    }
}

impl Source for DeckSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        CHANNELS
    }

    fn sample_rate(&self) -> SampleRate {
        SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// One open output device with the deck playing on it. Replaced whole when the
/// device drops out or the default changes: cpal pauses a stream for good,
/// with no error, when its device goes away (his AirBuds do this).
pub struct Output {
    _player: Player,
    _sink: MixerDeviceSink,
    pub device: Option<DeviceId>,
    pub device_name: String,
}

impl Output {
    pub fn open(deck: Arc<Mutex<Deck>>) -> Result<Self, Error> {
        let default = cpal::default_host().default_output_device();
        let mut sink = DeviceSinkBuilder::open_default_sink()?;
        sink.log_on_drop(false);

        let player = Player::connect_new(sink.mixer());
        player.append(DeckSource::new(deck));

        Ok(Self {
            _player: player,
            _sink: sink,
            device: default.as_ref().and_then(|d| d.id().ok()),
            device_name: default
                .and_then(|d| d.description().ok())
                .map(|d| d.name().to_string())
                .unwrap_or_default(),
        })
    }
}

pub fn default_device() -> Option<DeviceId> {
    cpal::default_host().default_output_device().and_then(|d| d.id().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One reading of `seconds` of a 220 Hz tone, ready to play.
    fn deck_with_tone(seconds: usize) -> Arc<Mutex<Deck>> {
        let mut deck = Deck::new(1.0, 1.0);
        let ids = deck.add_reading("t", vec!["tone".into()]);
        let tone: Vec<f32> = (0..RATE as usize * seconds)
            .map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5)
            .collect();
        deck.fill(ids[0], Some(Arc::from(vec![0u8])));
        deck.set_pcm(ids[0], Some(Arc::from(tone)));
        Arc::new(Mutex::new(deck))
    }

    fn pull(source: &mut DeckSource, seconds: f32) -> Vec<f32> {
        source.take((RATE as f32 * seconds) as usize).collect()
    }

    #[test]
    fn faster_speed_consumes_more_input_and_still_makes_sound() {
        let deck = deck_with_tone(10);
        lock(&deck).set_speed(1.5);
        let mut source = DeckSource::new(deck.clone());

        let out = pull(&mut source, 2.0);
        let position = lock(&deck).position().as_secs_f32();
        assert!((position - 3.0).abs() < 0.05, "read {position}s of input for 2s of output");

        let loud = out[RATE as usize..].iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!(loud > 0.2, "stretched output is near silent: peak {loud}");
    }

    #[test]
    fn normal_speed_passes_audio_untouched() {
        let deck = deck_with_tone(2);
        let expected: Vec<f32> = lock(&deck).current().unwrap().pcm.as_ref().unwrap()[..1000].to_vec();
        let mut source = DeckSource::new(deck);
        assert_eq!(pull(&mut source, 1000.0 / RATE as f32), expected);
    }

    #[test]
    fn paused_outputs_silence_without_moving() {
        let deck = deck_with_tone(2);
        lock(&deck).paused = true;
        let mut source = DeckSource::new(deck.clone());
        assert!(pull(&mut source, 0.5).iter().all(|s| *s == 0.0));
        assert_eq!(lock(&deck).position(), Duration::ZERO);
        assert!(lock(&deck).rendered > 0);
    }

    #[test]
    fn volume_scales_the_output() {
        let deck = deck_with_tone(1);
        lock(&deck).set_volume(0.5);
        let expected = lock(&deck).current().unwrap().pcm.as_ref().unwrap()[100] * 0.5;
        let mut source = DeckSource::new(deck);
        assert_eq!(pull(&mut source, 0.01)[100], expected);
    }
}
