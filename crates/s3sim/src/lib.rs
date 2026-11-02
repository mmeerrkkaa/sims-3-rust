//! Persistent, deterministic scheduling independent of Bevy entities and rendering.
pub mod residents;
pub mod mobility;
pub mod time;
use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};

/// One tick is 0.1 game minute. A date, not a count of rendered frames.
pub type Tick = u64;
pub type ResidentId = u64;

pub fn tick_at(minutes: f64) -> Tick {
    (minutes.max(0.0) * 10.0).floor() as Tick
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Agenda {
    due: BTreeMap<ResidentId, Tick>,
    last_tick: Option<Tick>,
    spent: usize,
}

#[derive(Debug, Default)]
pub struct Dispatch {
    pub residents: Vec<ResidentId>,
    pub overdue: usize,
    pub oldest_delay: Tick,
}

impl Agenda {
    /// Reconcile the active roster; reappearing residents receive staggered appointments.
    pub fn reconcile(&mut self, residents: impl IntoIterator<Item = ResidentId>, now: Tick, period: Tick) {
        assert!(period > 0);
        if self.last_tick.is_some_and(|t| now < t) {
            // A deliberate date rewind invalidates deadlines, not an unsigned wraparound.
            self.due.clear();
            self.last_tick = None;
            self.spent = 0;
        }
        let alive: BTreeSet<_> = residents.into_iter().collect();
        self.due.retain(|id, _| alive.contains(id));
        for id in alive {
            self.due.entry(id).or_insert_with(|| now.saturating_add(1 + phase(id) % period));
        }
    }

    /// Budget belongs to a game tick, so extra render frames cannot bypass it.
    /// This agenda is for coalescible decisions about current state, not historical events.
    pub fn dispatch(&mut self, now: Tick, period: Tick, budget: usize) -> Dispatch {
        assert!(period > 0);
        if self.last_tick != Some(now) {
            self.last_tick = Some(now);
            self.spent = 0;
        }
        let mut pending: Vec<_> = self.due.iter().filter(|(_, at)| **at <= now).map(|(id, at)| (*at, *id)).collect();
        pending.sort_unstable();
        let count = pending.len().min(budget.saturating_sub(self.spent));
        let residents = pending.iter().take(count).map(|(_, id)| *id).collect::<Vec<_>>();
        for id in &residents {
            self.due.insert(*id, now.saturating_add(period));
        }
        self.spent += count;
        Dispatch {
            residents,
            overdue: pending.len() - count,
            oldest_delay: pending.get(count).map_or(0, |(at, _)| now.saturating_sub(*at)),
        }
    }
}

fn phase(mut id: u64) -> u64 {
    id ^= id >> 30;
    id = id.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    id ^= id >> 27;
    id = id.wrapping_mul(0x94d0_49bb_1331_11eb);
    id ^ (id >> 31)
}

/// Topic cooldowns use stable IDs too, so save/load does not reset conversation history.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConversationPlans {
    pub agenda: Agenda,
    rested_until: BTreeMap<ResidentId, Tick>,
    said_until: BTreeMap<ResidentId, BTreeMap<String, Tick>>,
}

impl ConversationPlans {
    pub fn resting(&self, id: ResidentId, now: Tick) -> bool {
        self.rested_until.get(&id).is_some_and(|t| now < *t)
    }
    pub fn said(&self, id: ResidentId, name: &str, now: Tick) -> bool {
        self.said_until.get(&id).and_then(|s| s.get(name)).is_some_and(|t| now < *t)
    }
    pub fn remember(&mut self, id: ResidentId, name: &str, now: Tick, rest: Tick, repeat: Tick) {
        self.rested_until.insert(id, now.saturating_add(rest));
        self.said_until.entry(id).or_default().insert(name.into(), now.saturating_add(repeat));
    }
    pub fn expire(&mut self, now: Tick) {
        self.rested_until.retain(|_, t| *t > now);
        self.said_until.retain(|_, entries| { entries.retain(|_, t| *t > now); !entries.is_empty() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_survives_extra_frames_and_load_and_backlog_is_drained() {
        let mut a = Agenda::default();
        a.reconcile(1..=100, 0, 50);
        let first = a.dispatch(50, 50, 16);
        assert_eq!(first.residents.len(), 16);
        assert_eq!(first.overdue, 84);
        let mut a: Agenda = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert!(a.dispatch(50, 50, 16).residents.is_empty());
        let mut served: BTreeSet<_> = first.residents.into_iter().collect();
        for tick in 51..=56 { served.extend(a.dispatch(tick, 50, 16).residents); }
        assert_eq!(served.len(), 100);
        assert_eq!(a.dispatch(57, 50, 16).overdue, 0);
    }

    #[test]
    fn roster_order_and_render_frequency_do_not_change_dispatch() {
        let mut a = Agenda::default();
        let mut b = Agenda::default();
        a.reconcile(1..=30, 0, 50);
        b.reconcile((1..=30).rev(), 0, 50);
        for tick in 0..100 {
            assert_eq!(a.dispatch(tick, 50, 16).residents, b.dispatch(tick, 50, 16).residents);
            assert!(a.dispatch(tick, 50, 16).residents.is_empty());
        }
        a.reconcile([3], 2, 50);
        let got = a.dispatch(52, 50, 16);
        assert_eq!(got.residents, vec![3]);
    }

    #[test]
    fn cooldowns_roundtrip_and_expire_without_other_residents_interference() {
        let mut plans = ConversationPlans::default();
        plans.remember(42, "Chat", 100, 600, 14400);
        let mut loaded: ConversationPlans = serde_json::from_str(&serde_json::to_string(&plans).unwrap()).unwrap();
        assert!(loaded.resting(42, 699));
        assert!(!loaded.resting(42, 700));
        assert!(loaded.said(42, "Chat", 700));
        assert!(!loaded.said(43, "Chat", 700));
        loaded.expire(14500);
        assert!(!loaded.said(42, "Chat", 14500));
        assert!(loaded.said_until.is_empty());
    }
}
