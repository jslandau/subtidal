pub const FEED: usize = 2560;
pub const ASR: usize = 8960;
pub const RATE: usize = 16000;
#[derive(Default)]
pub struct Framer {
    pending: Vec<f32>,
    pub consumed: usize,
}
impl Framer {
    pub fn push(&mut self, pcm: &[f32]) -> Option<Vec<f32>> {
        self.consumed += pcm.len();
        self.pending.extend_from_slice(pcm);
        if self.pending.len() >= ASR {
            Some(self.pending.drain(..ASR).collect())
        } else {
            None
        }
    }
    pub fn finish(&mut self) -> Option<Vec<f32>> {
        if self.pending.is_empty() {
            return None;
        }
        let mut pcm = std::mem::take(&mut self.pending);
        pcm.resize(ASR, 0.0);
        Some(pcm)
    }
    pub fn reset(&mut self) {
        self.pending.clear();
        self.consumed = 0;
    }
}
pub fn clip_segment(
    start: usize,
    end: usize,
    id: usize,
    real: usize,
) -> Option<(usize, usize, usize)> {
    let end = end.min(real);
    if id < 8 && start < end {
        Some((start, end, id))
    } else {
        None
    }
}
pub fn rtf(seconds: f64, samples: usize) -> f64 {
    seconds / (samples as f64 / RATE as f64)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_and_single_finish() {
        let mut f = Framer::default();
        for _ in 0..3 {
            assert!(f.push(&vec![1.; FEED]).is_none());
        }
        assert_eq!(f.push(&vec![2.; FEED]).unwrap().len(), ASR);
        let tail = f.finish().unwrap();
        assert_eq!(tail[0], 2.);
        assert_eq!(tail[1280], 0.);
        assert!(f.finish().is_none());
        assert_eq!(f.consumed, 10240);
    }
    #[test]
    fn reset_offsets() {
        let mut f = Framer::default();
        f.push(&[1.; 123]);
        f.reset();
        assert_eq!(f.consumed, 0);
        assert!(f.finish().is_none());
    }
    #[test]
    fn absolute_offsets_and_ids() {
        assert_eq!(clip_segment(100, 200, 7, 150), Some((100, 150, 7)));
        assert!(clip_segment(150, 200, 0, 150).is_none());
        assert!(clip_segment(0, 100, 8, 150).is_none());
    }
    #[test]
    fn timing_real_audio() {
        assert_eq!(rtf(2., 16000), 2.);
    }
}
