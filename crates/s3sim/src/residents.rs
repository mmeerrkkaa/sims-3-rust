//! Persistent spatial state uses world lot IDs, never ECS entities or array indices.
use crate::Tick;
use serde::{Deserialize, Serialize};

pub type LotId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldLocation {
    AtLot { lot: LotId },
    ToLot { lot: LotId, arrive_at: Tick },
    ToHome { arrive_at: Tick },
}

/// Detailed-lot ownership is persistent, independent of the ECS actor's current markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LotRole {
    Guest { leave_at: Tick },
    Resident,
}


/// A resident's persistent home and last checkpoint. Detailed actors are authoritative
/// while instantiated; this record owns state when there is no detailed actor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resident<T> {
    pub household: Option<u64>,
    pub home: Option<LotId>,
    pub state: T,
    pub checkpoint_at: Option<Tick>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Residents<T> {
    pub records: std::collections::BTreeMap<crate::ResidentId, Resident<T>>,
}

impl<T> Default for Residents<T> {
    fn default() -> Self { Self { records: Default::default() } }
}

impl<T> Residents<T> {
    /// Importing the world's roster must never overwrite gameplay checkpoints.
    pub fn seed(&mut self, id: crate::ResidentId, household: Option<u64>, home: Option<LotId>, state: T) {
        self.records.entry(id).or_insert(Resident { household, home, state, checkpoint_at: None });
    }

    pub fn checkpoint(&mut self, id: crate::ResidentId, state: T, now: Tick) {
        use std::collections::btree_map::Entry;
        match self.records.entry(id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().state = state;
                entry.get_mut().checkpoint_at = Some(now);
            }
            Entry::Vacant(entry) => { entry.insert(Resident { household: None, home: None, state, checkpoint_at: Some(now) }); }
        }
    }
}

/// Street journeys contain world coordinates rather than ECS path/reservation handles.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreetWalk {
    pub destination: [f32; 2],
    pub speed: f32,
    pub jogging: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackgroundState {
    pub next_walk: f64,
    pub walk: Option<StreetWalk>,
}


#[cfg(test)]
mod store_tests {
    use super::*;
    #[test]
    fn checkpoint_survives_roster_import_and_roundtrip_without_scene_entities() {
        let mut residents = Residents::default();
        for id in 1..=1000 { residents.seed(id, Some(id / 4), Some(id / 8), vec![0_u32]); }
        residents.checkpoint(42, vec![7, 9], 1234);
        residents.seed(42, Some(9999), Some(9999), vec![0]);
        let loaded: Residents<Vec<u32>> = serde_json::from_slice(&serde_json::to_vec(&residents).unwrap()).unwrap();
        assert_eq!(loaded.records.len(), 1000);
        let record = &loaded.records[&42];
        assert_eq!(record.household, Some(10)); assert_eq!(record.home, Some(5));
        assert_eq!(record.state, vec![7, 9]); assert_eq!(record.checkpoint_at, Some(1234));
        assert_eq!(loaded.records[&43].checkpoint_at, None);
    }
}
