//! Statistics for one reporting window, independent of rendering and game assets.

#[derive(Default)]
pub struct Samples {
    frames_ms: Vec<f64>,
    total_ms: f64,
    update_ms: f64,
}

pub struct Summary {
    pub frames: usize,
    pub seconds: f64,
    pub fps: f64,
    pub mean_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub update_mean_ms: f64,
}

impl Samples {
    pub fn push(&mut self, frame_ms: f64, update_ms: f64) {
        self.frames_ms.push(frame_ms);
        self.total_ms += frame_ms;
        self.update_ms += update_ms;
    }

    pub fn seconds(&self) -> f64 {
        self.total_ms / 1000.0
    }

    pub fn take_summary(&mut self) -> Option<Summary> {
        let frames = self.frames_ms.len();
        if frames == 0 {
            return None;
        }
        let seconds = self.seconds();
        self.frames_ms.sort_unstable_by(f64::total_cmp);
        let summary = Summary {
            frames,
            seconds,
            fps: if seconds > 0.0 {
                frames as f64 / seconds
            } else {
                0.0
            },
            mean_ms: seconds * 1000.0 / frames as f64,
            p95_ms: self.frames_ms[(frames * 95).div_ceil(100) - 1],
            max_ms: self.frames_ms[frames - 1],
            update_mean_ms: self.update_ms / frames as f64,
        };
        self.frames_ms.clear();
        self.total_ms = 0.0;
        self.update_ms = 0.0;
        Some(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_windows_have_no_report() {
        assert!(Samples::default().take_summary().is_none());
    }

    #[test]
    fn reports_hitches_and_time_weighted_fps() {
        let mut samples = Samples::default();
        for _ in 0..19 {
            samples.push(10.0, 2.0);
        }
        samples.push(110.0, 12.0);
        let s = samples.take_summary().unwrap();
        assert_eq!(s.frames, 20);
        assert!((s.seconds - 0.3).abs() < 1e-9);
        assert!((s.fps - 20.0 / 0.3).abs() < 1e-9);
        assert_eq!(s.mean_ms, 15.0);
        assert_eq!(s.p95_ms, 10.0);
        assert_eq!(s.max_ms, 110.0);
        assert_eq!(s.update_mean_ms, 2.5);
        assert!(samples.take_summary().is_none());
        samples.push(20.0, 3.0);
        let next = samples.take_summary().unwrap();
        assert_eq!(next.fps, 50.0);
        assert_eq!(next.p95_ms, 20.0);
        assert_eq!(next.update_mean_ms, 3.0);
    }
}
