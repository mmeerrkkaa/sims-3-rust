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
