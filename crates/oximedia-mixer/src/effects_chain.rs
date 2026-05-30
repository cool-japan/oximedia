//! Effects chain management for the audio mixer.

// ---------------------------------------------------------------------------
// AudioEffect trait
// ---------------------------------------------------------------------------

/// A single audio effect that can process a mono sample buffer in-place.
pub trait AudioEffect: Send + Sync {
    /// Process the sample buffer in-place.
    fn process(&mut self, samples: &mut [f32]);

    /// Human-readable effect name.
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Delay
// ---------------------------------------------------------------------------

/// Tape-style delay / echo effect.
#[derive(Debug, Clone)]
pub struct DelayEffect {
    /// Delay length in samples.
    pub delay_samples: usize,
    /// Feedback factor (0.0 = no feedback, <1.0 to avoid runaway).
    pub feedback: f32,
    /// Wet/dry mix (0.0 = dry, 1.0 = full wet).
    pub mix: f32,
    /// Internal circular buffer.
    buffer: Vec<f32>,
    /// Current write position in the buffer.
    write_pos: usize,
}

impl DelayEffect {
    /// Create a new delay effect.
    ///
    /// The internal buffer is allocated to `delay_samples` (minimum 1).
    #[must_use]
    pub fn new(delay_samples: usize, feedback: f32, mix: f32) -> Self {
        let len = delay_samples.max(1);
        Self {
            delay_samples: len,
            feedback: feedback.clamp(0.0, 0.999),
            mix: mix.clamp(0.0, 1.0),
            buffer: vec![0.0_f32; len],
            write_pos: 0,
        }
    }
}

impl AudioEffect for DelayEffect {

    const EFFECT_ID: u64 = 6034;
    fn process(&mut self, samples: &mut [f32]) {
        let buf_len = self.buffer.len();
        for sample in samples.iter_mut() {
            // Read delayed sample
            let read_pos =
                (self.write_pos + buf_len - self.delay_samples.min(buf_len - 1)) % buf_len;
            let delayed = self.buffer[read_pos];

            // Write current + feedback into buffer
            self.buffer[self.write_pos] = *sample + delayed * self.feedback;
            self.write_pos = (self.write_pos + 1) % buf_len;

            // Mix dry and wet
            *sample = *sample * (1.0 - self.mix) + delayed * self.mix;
        }
    }

    fn name(&self) -> &'static str {
        "Delay"
    }
}

// ---------------------------------------------------------------------------
// Chorus
// ---------------------------------------------------------------------------

/// LFO-modulated chorus effect using an all-pass delay line.
///
/// The LFO sweeps the read position within a delay buffer, producing
/// pitch modulation between the dry signal and the delayed copy.
/// This implements the classic "Haas / flanging" model: one modulated
/// delay tap mixed with the dry signal.
#[derive(Debug, Clone)]
pub struct ChorusEffect {
    /// LFO rate in Hz.
    pub rate_hz: f32,
    /// Peak modulation depth in samples (max read-offset swing around the centre delay).
    pub depth_samples: f32,
    /// Wet/dry mix (0.0 = dry, 1.0 = full wet).
    pub mix: f32,
    /// Current LFO phase (radians).
    lfo_phase: f32,
    /// Circular delay buffer — length = `centre_delay + depth_samples` rounded up, minimum 8.
    delay_buffer: Vec<f32>,
    /// Current write position in the delay buffer.
    write_pos: usize,
}

impl ChorusEffect {
    /// Default sample rate assumed when not provided externally.
    const SAMPLE_RATE: f32 = 48_000.0;
    /// Centre delay around which the LFO modulates (in samples at 48 kHz).
    const CENTRE_DELAY_SAMPLES: f32 = 720.0; // ~15 ms

    /// Create a new chorus effect.
    ///
    /// * `rate_hz` — LFO rate (Hz), clamped to ≥ 0.
    /// * `depth_samples` — peak modulation depth in samples, clamped to ≥ 0.
    /// * `mix` — wet/dry blend \[0, 1\].
    #[must_use]
    pub fn new(rate_hz: f32, depth_samples: f32, mix: f32) -> Self {
        let depth = depth_samples.max(0.0);
        let buf_len = ((Self::CENTRE_DELAY_SAMPLES + depth).ceil() as usize + 2).max(8);
        Self {
            rate_hz: rate_hz.max(0.0),
            depth_samples: depth,
            mix: mix.clamp(0.0, 1.0),
            lfo_phase: 0.0,
            delay_buffer: vec![0.0_f32; buf_len],
            write_pos: 0,
        }
    }
}

impl AudioEffect for ChorusEffect {

    const EFFECT_ID: u64 = 6035;
    /// Process samples with LFO-modulated delay-line chorus.
    ///
    /// For each input sample:
    /// 1. Write the dry sample into the circular buffer.
    /// 2. Compute the fractional read offset: `centre + depth * sin(lfo_phase)`.
    /// 3. Linear-interpolate between the two surrounding buffer slots.
    /// 4. Advance the LFO phase by `TAU * rate_hz / SAMPLE_RATE`.
    /// 5. Mix dry and wet.
    fn process(&mut self, samples: &mut [f32]) {
        use std::f32::consts::TAU;

        let buf_len = self.delay_buffer.len();
        let lfo_step = TAU * self.rate_hz / Self::SAMPLE_RATE;

        for sample in samples.iter_mut() {
            // Write dry sample into delay buffer.
            self.delay_buffer[self.write_pos] = *sample;

            // Modulated delay offset (in samples), always ≥ 1 to avoid reading
            // the sample we just wrote.
            let mod_offset = Self::CENTRE_DELAY_SAMPLES + self.depth_samples * self.lfo_phase.sin();
            let offset_floor = mod_offset.floor() as usize;
            let frac = mod_offset - mod_offset.floor();

            // Clamp offsets within valid buffer range.
            let offset_a = offset_floor.min(buf_len - 1);
            let offset_b = (offset_floor + 1).min(buf_len - 1);

            let read_a = (self.write_pos + buf_len - offset_a) % buf_len;
            let read_b = (self.write_pos + buf_len - offset_b) % buf_len;

            // Linear interpolation between the two tap samples.
            let delayed =
                self.delay_buffer[read_a] * (1.0 - frac) + self.delay_buffer[read_b] * frac;

            // Advance buffer write position.
            self.write_pos = (self.write_pos + 1) % buf_len;

            // Advance LFO phase.
            self.lfo_phase = (self.lfo_phase + lfo_step) % TAU;

            // Mix dry and wet.
            *sample = *sample * (1.0 - self.mix) + delayed * self.mix;
        }
    }

    fn name(&self) -> &'static str {
        "Chorus"
    }
}

// ---------------------------------------------------------------------------
// Reverb
// ---------------------------------------------------------------------------

/// Schroeder reverb with 4 parallel comb filters followed by 2 all-pass filters in series.
///
/// Architecture (classic Schroeder 1962 / Moorer 1979 topology):
///
/// ```text
///                     ┌─ comb0 (LP-damped) ─┐
///                     ├─ comb1 (LP-damped) ─┤
///  input ─────────────┤                      ├──(sum)── allpass0 ── allpass1 ── wet
///                     ├─ comb2 (LP-damped) ─┤
///                     └─ comb3 (LP-damped) ─┘
/// ```
///
/// The four comb filters run in parallel; their outputs are averaged and then
/// passed through two all-pass diffusers before being mixed with the dry signal.
#[derive(Debug, Clone)]
pub struct ReverbEffect {
    /// Room size factor (0.0–1.0 controls comb filter delays and feedback).
    pub room_size: f32,
    /// High-frequency damping (0.0 = bright, 1.0 = dark).
    pub damping: f32,
    /// Wet/dry mix.
    pub mix: f32,
    /// Comb filter buffers (one per filter).
    comb_buffers: Vec<Vec<f32>>,
    /// Write positions for each comb filter.
    comb_positions: Vec<usize>,
    /// Low-pass filter states for HF damping inside each comb filter.
    lp_states: Vec<f32>,
    /// All-pass filter buffers (2 filters).
    ap_buffers: [Vec<f32>; 2],
    /// Write positions for each all-pass filter.
    ap_positions: [usize; 2],
}

impl ReverbEffect {
    /// Comb filter base delay lengths (samples at 44100 Hz), scaled by `room_size`.
    const COMB_BASE_DELAYS: [usize; 4] = [1557, 1617, 1491, 1422];
    /// All-pass filter fixed delay lengths (samples at 44100 Hz).
    const AP_DELAYS: [usize; 2] = [225, 556];
    /// All-pass filter gain coefficient (Schroeder standard: 0.5).
    const AP_GAIN: f32 = 0.5;

    /// Create a new reverb effect.
    ///
    /// * `room_size` — \[0, 1\]: larger values → longer delays and more feedback.
    /// * `damping`   — \[0, 1\]: 0 = bright (no HF roll-off), 1 = very dark.
    /// * `mix`       — \[0, 1\]: wet/dry blend.
    #[must_use]
    pub fn new(room_size: f32, damping: f32, mix: f32) -> Self {
        let rs = room_size.clamp(0.0, 1.0);

        let comb_buffers: Vec<Vec<f32>> = Self::COMB_BASE_DELAYS
            .iter()
            .map(|&d| {
                let len = ((d as f32 * (0.5 + rs * 0.5)) as usize).max(1);
                vec![0.0_f32; len]
            })
            .collect();
        let n_comb = comb_buffers.len();

        let ap_buffers = [
            vec![0.0_f32; Self::AP_DELAYS[0].max(1)],
            vec![0.0_f32; Self::AP_DELAYS[1].max(1)],
        ];

        Self {
            room_size: rs,
            damping: damping.clamp(0.0, 1.0),
            mix: mix.clamp(0.0, 1.0),
            comb_buffers,
            comb_positions: vec![0; n_comb],
            lp_states: vec![0.0_f32; n_comb],
            ap_buffers,
            ap_positions: [0; 2],
        }
    }

    /// Process one sample through a single all-pass diffuser.
    ///
    /// The all-pass transfer function is:
    ///   `H(z) = (-g + z^{-N}) / (1 - g * z^{-N})`
    ///
    /// with `g = AP_GAIN`.  This colours the echo density without colouring
    /// the frequency response (flat magnitude).
    #[inline]
    fn allpass_tick(buf: &mut Vec<f32>, pos: &mut usize, input: f32) -> f32 {
        let len = buf.len();
        let delayed = buf[*pos];
        let feedback = input + delayed * Self::AP_GAIN;
        buf[*pos] = feedback;
        *pos = (*pos + 1) % len;
        delayed - input * Self::AP_GAIN
    }
}

impl AudioEffect for ReverbEffect {
    const EFFECT_ID: u64 = 6036;

    /// Process samples through the Schroeder reverb network.
    fn process(&mut self, samples: &mut [f32]) {
        // Feedback coefficient: scales with room size, capped well below 1.0
        // to guarantee stability.
        let feedback = (0.7_f32 + 0.28 * self.room_size).min(0.98);
        let damp = self.damping;

        for sample in samples.iter_mut() {
            let input = *sample;
            let mut comb_sum = 0.0_f32;

            // ── 4 parallel LP-damped comb filters ────────────────────────────
            for i in 0..self.comb_buffers.len() {
                let len = self.comb_buffers[i].len();
                let pos = self.comb_positions[i];
                let delayed = self.comb_buffers[i][pos];

                // One-pole low-pass inside the feedback loop (Moorer damping).
                self.lp_states[i] = delayed * (1.0 - damp) + self.lp_states[i] * damp;
                self.comb_buffers[i][pos] = input + self.lp_states[i] * feedback;
                self.comb_positions[i] = (pos + 1) % len;

                comb_sum += delayed;
            }

            #[allow(clippy::cast_precision_loss)]
            let mut wet = comb_sum / self.comb_buffers.len() as f32;

            // ── 2 all-pass diffusers in series ────────────────────────────────
            wet = Self::allpass_tick(&mut self.ap_buffers[0], &mut self.ap_positions[0], wet);
            wet = Self::allpass_tick(&mut self.ap_buffers[1], &mut self.ap_positions[1], wet);

            *sample = input * (1.0 - self.mix) + wet * self.mix;
        }
    }

    fn name(&self) -> &'static str {
        "Reverb"
    }
}

// ---------------------------------------------------------------------------
// Effects Chain
// ---------------------------------------------------------------------------

/// An ordered chain of audio effects applied in sequence.
pub struct EffectsChain {
    effects: Vec<Box<dyn AudioEffect>>,
}

impl EffectsChain {
    /// Create an empty effects chain.
    #[must_use]
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    /// Append an effect to the end of the chain.
    pub fn add(&mut self, effect: Box<dyn AudioEffect>) {
        self.effects.push(effect);
    }

    /// Remove the effect at `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= self.len()`.
    pub fn remove(&mut self, idx: usize) {
        self.effects.remove(idx);
    }

    /// Process a sample buffer through every effect in order.
    pub fn process_block(&mut self, samples: &mut [f32]) {
        for effect in &mut self.effects {
            effect.process(samples);
        }
    }

    /// Number of effects in the chain.
    #[must_use]
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Returns `true` if the chain contains no effects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

impl Default for EffectsChain {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delay_effect_name() {
        let d = DelayEffect::new(100, 0.5, 0.5);
        assert_eq!(d.name(), "Delay");
    }

    #[test]
    fn test_delay_dry_signal_when_mix_zero() {
        let mut d = DelayEffect::new(10, 0.0, 0.0);
        let mut samples = [0.5_f32, 0.3, 0.1];
        let original = samples;
        d.process(&mut samples);
        for (s, o) in samples.iter().zip(original.iter()) {
            assert!((s - o).abs() < 1e-6, "s={s} o={o}");
        }
    }

    #[test]
    fn test_delay_buffer_length_minimum_one() {
        let d = DelayEffect::new(0, 0.5, 0.5);
        assert_eq!(d.buffer.len(), 1);
    }

    #[test]
    fn test_delay_feedback_clamp() {
        let d = DelayEffect::new(10, 1.5, 0.5);
        assert!(d.feedback < 1.0);
    }

    #[test]
    fn test_chorus_effect_name() {
        let c = ChorusEffect::new(1.5, 5.0, 0.3);
        assert_eq!(c.name(), "Chorus");
    }

    #[test]
    fn test_chorus_does_not_blow_up() {
        let mut c = ChorusEffect::new(1.5, 5.0, 0.5);
        let mut samples = vec![0.1_f32; 512];
        c.process(&mut samples);
        for s in &samples {
            assert!(s.is_finite(), "non-finite sample after chorus");
        }
    }

    #[test]
    fn test_reverb_effect_name() {
        let r = ReverbEffect::new(0.5, 0.5, 0.3);
        assert_eq!(r.name(), "Reverb");
    }

    #[test]
    fn test_reverb_does_not_blow_up() {
        let mut r = ReverbEffect::new(0.7, 0.5, 0.4);
        let mut samples = vec![0.1_f32; 1024];
        r.process(&mut samples);
        for s in &samples {
            assert!(s.is_finite(), "non-finite sample after reverb");
        }
    }

    #[test]
    fn test_effects_chain_starts_empty() {
        let chain = EffectsChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn test_effects_chain_add() {
        let mut chain = EffectsChain::new();
        chain.add(Box::new(DelayEffect::new(100, 0.3, 0.5)));
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());
    }

    #[test]
    fn test_effects_chain_remove() {
        let mut chain = EffectsChain::new();
        chain.add(Box::new(DelayEffect::new(100, 0.3, 0.5)));
        chain.add(Box::new(ChorusEffect::new(1.0, 3.0, 0.3)));
        chain.remove(0);
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn test_effects_chain_process_block_passthrough() {
        // An empty chain should leave samples unchanged.
        let mut chain = EffectsChain::new();
        let mut samples = [0.1_f32, 0.2, 0.3, 0.4];
        let original = samples;
        chain.process_block(&mut samples);
        for (s, o) in samples.iter().zip(original.iter()) {
            assert!((s - o).abs() < 1e-6);
        }
    }

    #[test]
    fn test_effects_chain_default_is_empty() {
        let chain: EffectsChain = Default::default();
        assert!(chain.is_empty());
    }
}
