pub fn map_channels(
    frames: &[f32],
    input_channels: usize,
    output_channels: usize,
    out: &mut Vec<f32>,
) {
    if input_channels == 0 || output_channels == 0 {
        return;
    }
    if input_channels == output_channels {
        out.extend_from_slice(frames);
        return;
    }
    for frame in frames.chunks_exact(input_channels) {
        match (input_channels, output_channels) {
            (1, _) => {
                for _ in 0..output_channels {
                    out.push(frame[0]);
                }
            }
            (_, 1) => {
                let sum: f32 = frame.iter().sum();
                out.push(sum / input_channels as f32);
            }
            _ => {
                for channel in 0..output_channels {
                    out.push(frame[channel.min(input_channels - 1)]);
                }
            }
        }
    }
}

pub struct LinearResampler {
    channels: usize,
    ratio: f64,
    position: f64,
    previous: Vec<f32>,
    primed: bool,
}

impl LinearResampler {
    pub fn new(input_rate: u32, output_rate: u32, channels: usize) -> Self {
        Self {
            channels,
            ratio: input_rate as f64 / output_rate as f64,
            position: 0.0,
            previous: vec![0.0; channels],
            primed: false,
        }
    }

    pub fn is_identity(&self) -> bool {
        (self.ratio - 1.0).abs() < f64::EPSILON
    }

    pub fn reset(&mut self) {
        self.position = 0.0;
        self.previous.iter_mut().for_each(|sample| *sample = 0.0);
        self.primed = false;
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.channels == 0 || input.is_empty() {
            return;
        }
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }

        let frames = input.len() / self.channels;
        if frames == 0 {
            return;
        }
        let input = &input[..frames * self.channels];
        if !self.primed {
            self.previous.copy_from_slice(&input[..self.channels]);
            self.primed = true;
        }

        while self.position < frames as f64 {
            let index = self.position.floor() as usize;
            let fraction = (self.position - index as f64) as f32;

            for channel in 0..self.channels {
                let earlier = if index == 0 {
                    self.previous[channel]
                } else {
                    input[(index - 1) * self.channels + channel]
                };
                let later = input[index * self.channels + channel];
                out.push(earlier + (later - earlier) * fraction);
            }
            self.position += self.ratio;
        }

        self.previous
            .copy_from_slice(&input[(frames - 1) * self.channels..frames * self.channels]);
        self.position -= frames as f64;
    }
}

pub fn replay_gain_scale(gain_db: Option<f32>, peak: Option<f32>) -> f32 {
    let Some(gain) = gain_db else {
        return 1.0;
    };
    let mut scale = 10f32.powf(gain / 20.0);
    if let Some(peak) = peak.filter(|peak| *peak > 0.0)
        && scale * peak > 1.0
    {
        scale = 1.0 / peak;
    }
    scale.clamp(0.0, 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_layouts_pass_through_untouched() {
        let mut out = Vec::new();
        map_channels(&[1.0, 2.0, 3.0, 4.0], 2, 2, &mut out);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn mono_is_duplicated_into_every_output_channel() {
        let mut out = Vec::new();
        map_channels(&[1.0, 2.0], 1, 2, &mut out);
        assert_eq!(out, vec![1.0, 1.0, 2.0, 2.0]);
    }

    #[test]
    fn stereo_folds_down_to_mono_by_averaging() {
        let mut out = Vec::new();
        map_channels(&[1.0, 3.0, 2.0, 4.0], 2, 1, &mut out);
        assert_eq!(out, vec![2.0, 3.0]);
    }

    #[test]
    fn surround_is_truncated_to_the_available_channels() {
        let mut out = Vec::new();
        map_channels(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6, 2, &mut out);
        assert_eq!(out, vec![1.0, 2.0]);
    }

    #[test]
    fn a_matching_rate_needs_no_resampling() {
        let mut resampler = LinearResampler::new(44_100, 44_100, 2);
        assert!(resampler.is_identity());
        let mut out = Vec::new();
        resampler.process(&[1.0, 2.0, 3.0, 4.0], &mut out);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn doubling_the_rate_roughly_doubles_the_frame_count() {
        let mut resampler = LinearResampler::new(22_050, 44_100, 1);
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut out = Vec::new();
        resampler.process(&input, &mut out);
        assert!((out.len() as i64 - 200).abs() <= 2, "{} frames", out.len());
    }

    #[test]
    fn halving_the_rate_roughly_halves_the_frame_count() {
        let mut resampler = LinearResampler::new(96_000, 48_000, 2);
        let input: Vec<f32> = (0..400).map(|i| i as f32).collect();
        let mut out = Vec::new();
        resampler.process(&input, &mut out);
        let frames = out.len() / 2;
        assert!((frames as i64 - 100).abs() <= 2, "{frames} frames");
    }

    #[test]
    fn resampling_stays_continuous_across_successive_blocks() {
        let mut resampler = LinearResampler::new(48_000, 96_000, 1);
        let mut out = Vec::new();
        resampler.process(&[0.0, 1.0, 2.0, 3.0], &mut out);
        let first = out.len();
        resampler.process(&[4.0, 5.0, 6.0, 7.0], &mut out);
        assert!(out.len() > first);
        assert!(out.windows(2).all(|pair| pair[1] >= pair[0] - 0.001));
    }

    #[test]
    fn a_block_short_of_a_whole_frame_is_left_alone_rather_than_panicking() {
        let mut resampler = LinearResampler::new(44_100, 48_000, 2);
        let mut out = Vec::new();
        resampler.process(&[0.5], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_trailing_partial_frame_does_not_disturb_the_whole_ones() {
        let mut resampler = LinearResampler::new(48_000, 48_000 * 2, 2);
        let mut out = Vec::new();
        resampler.process(&[1.0, 1.0, 2.0, 2.0, 3.0], &mut out);
        assert!(!out.is_empty());
        assert_eq!(out.len() % 2, 0);
    }

    #[test]
    fn resetting_clears_the_interpolation_state() {
        let mut resampler = LinearResampler::new(48_000, 44_100, 2);
        let mut out = Vec::new();
        resampler.process(&[1.0, 1.0, 2.0, 2.0], &mut out);
        resampler.reset();
        out.clear();
        resampler.process(&[5.0, 5.0], &mut out);
        assert_eq!(out[0], 5.0);
    }

    #[test]
    fn absent_replay_gain_leaves_the_signal_alone() {
        assert_eq!(replay_gain_scale(None, None), 1.0);
    }

    #[test]
    fn negative_replay_gain_attenuates() {
        let scale = replay_gain_scale(Some(-6.0), None);
        assert!(scale < 1.0 && scale > 0.49, "{scale}");
    }

    #[test]
    fn peak_information_prevents_clipping() {
        let scale = replay_gain_scale(Some(12.0), Some(0.98));
        assert!(scale * 0.98 <= 1.0001, "{scale}");
    }

    #[test]
    fn absurd_gain_is_clamped() {
        assert!(replay_gain_scale(Some(60.0), None) <= 4.0);
    }
}
