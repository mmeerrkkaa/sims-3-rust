//! Fixed wall-time slices with the speed attached when time was admitted.
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, time::Duration};

pub const STEP_NS: u64 = 33_333_333;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Timeline {
    segments: VecDeque<Segment>,
    pub ticks: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Segment {
    nanos: u64,
    rate: u32,
}

impl Timeline {
    pub fn admit(&mut self, elapsed: Duration, rate: u32) {
        if rate == 0 || elapsed.is_zero() {
            return;
        }
        let nanos = u64::try_from(elapsed.as_nanos()).expect("elapsed duration fits timeline");
        if let Some(last) = self.segments.back_mut().filter(|s| s.rate == rate) {
            last.nanos = last.nanos.checked_add(nanos).expect("timeline capacity");
        } else {
            self.segments.push_back(Segment { nanos, rate });
        }
    }

    pub fn pending_seconds(&self) -> f64 {
        self.segments.iter().map(|s| s.nanos as f64 / 1e9).sum()
    }

    pub fn pending_minutes(&self) -> f64 {
        self.segments
            .iter()
            .map(|s| s.nanos as f64 * s.rate as f64 / 1e9)
            .sum()
    }

    /// Consume one equal wall-time slice. Speed changes retain the old queued work's rate.
    pub fn pop(&mut self) -> Option<f64> {
        if self.segments.iter().map(|s| s.nanos as u128).sum::<u128>() < STEP_NS as u128 {
            return None;
        }
        let mut remaining = STEP_NS;
        let mut minutes = 0.0;
        while remaining > 0 {
            let segment = self.segments.front_mut().unwrap();
            let consumed = remaining.min(segment.nanos);
            minutes += consumed as f64 * segment.rate as f64 / 1e9;
            remaining -= consumed;
            segment.nanos -= consumed;
            if segment.nanos == 0 {
                self.segments.pop_front();
            }
        }
        self.ticks += 1;
        Some(minutes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_frame_budget_save_and_speed_change_keep_all_time() {
        let mut t = Timeline::default();
        t.admit(Duration::from_secs(1), 12);
        let mut simulated = (0..4).map(|_| t.pop().unwrap()).sum::<f64>();
        assert!(t.pending_seconds() > 0.86);
        t = serde_json::from_slice(&serde_json::to_vec(&t).unwrap()).unwrap();
        t.admit(Duration::from_millis(500), 1);
        t.admit(Duration::from_secs(10), 0);
        while let Some(minutes) = t.pop() {
            simulated += minutes;
        }
        assert!((simulated + t.pending_minutes() - 12.5).abs() < 1e-9);
        assert!(t.pending_seconds() < STEP_NS as f64 / 1e9);
    }

    #[test]
    fn rendering_frequency_does_not_change_tick_results() {
        let mut a = Timeline::default();
        let mut b = Timeline::default();
        a.admit(Duration::from_secs(1), 3);
        let mut from_a = Vec::new();
        while let Some(step) = a.pop() {
            from_a.push(step);
        }
        let mut from_b = Vec::new();
        for _ in 0..100 {
            b.admit(Duration::from_millis(10), 3);
            while let Some(step) = b.pop() {
                from_b.push(step);
            }
        }
        assert_eq!(from_a.len(), from_b.len());
        assert!(
            from_a
                .iter()
                .zip(from_b)
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        assert_eq!(a.ticks, b.ticks);
    }
}
