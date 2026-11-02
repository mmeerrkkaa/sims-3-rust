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


/// The city model is stored with identity, not in an unrelated parallel roster.
/// Personal checkpoints retain detailed needs/skills/inventory; these fields also
/// exist for residents who have never had a scene actor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResidentModel {
    pub age: Option<String>,
    pub days: f32,
    pub dead: bool,
    pub gone: bool,
    pub partner: Option<crate::ResidentId>,
    pub spouse: Option<crate::ResidentId>,
    /// Career name and one-based level.
    pub job: Option<(String, u32)>,
    pub elder_span: f32,
    pub elder_risk: Option<f64>,
    /// Existing played saves without survival state used a fixed old-age span.
    pub legacy_fixed_lifespan: bool,
}

impl Default for ResidentModel {
    fn default() -> Self {
        Self { age: None, days: 0.0, dead: false, gone: false, partner: None,
            spouse: None, job: None, elder_span: 17.0, elder_risk: None,
            legacy_fixed_lifespan: false }
    }
}

impl ResidentModel {
    pub fn present(&self) -> bool { !self.dead && !self.gone }
}

/// A stable survival threshold is sampled once, independent of scene/load order.
pub fn initial_elder_risk(id: crate::ResidentId) -> f64 {
    let uniform = (crate::phase(id) >> 11).max(1) as f64 / 9_007_199_254_740_992.0;
    -uniform.ln()
}

/// Common survival calculation for detailed and background aging. A missing
/// threshold retains the fixed-span behavior of legacy played households.
pub fn old_age_today(days: f32, span: f32, risk: &mut Option<f64>, probability: f64, minimum: f32) -> bool {
    let Some(risk) = risk.as_mut() else { return days >= span };
    if days < minimum { return false; }
    *risk += (1.0 - probability).ln();
    *risk <= 0.0
}

/// Records may arrive from an old daily-model save before their appearance/profile
/// is available. No placeholder Sim or scene model is invented for those IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resident<T> {
    pub household: Option<u64>,
    pub home: Option<LotId>,
    pub state: Option<T>,
    pub checkpoint_at: Option<Tick>,
    pub model: Option<ResidentModel>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Residents<T> {
    pub records: std::collections::BTreeMap<crate::ResidentId, Resident<T>>,
}

impl<T> Default for Residents<T> {
    fn default() -> Self { Self { records: Default::default() } }
}

impl<T> Residents<T> {
    fn entry(&mut self, id: crate::ResidentId) -> &mut Resident<T> {
        self.records.entry(id).or_insert(Resident { household: None, home: None,
            state: None, checkpoint_at: None, model: None })
    }

    /// Roster import fills missing profiles without overwriting gameplay state.
    pub fn seed(&mut self, id: crate::ResidentId, household: Option<u64>, home: Option<LotId>, state: T) {
        let resident = self.entry(id);
        if resident.household.is_none() { resident.household = household; resident.home = home; }
        if resident.state.is_none() { resident.state = Some(state); }
    }

    pub fn checkpoint(&mut self, id: crate::ResidentId, state: T, now: Tick) {
        let resident = self.entry(id);
        resident.state = Some(state);
        resident.checkpoint_at = Some(now);
    }

    pub fn model(&self, id: &crate::ResidentId) -> Option<&ResidentModel> {
        self.records.get(id)?.model.as_ref()
    }

    pub fn model_mut(&mut self, id: &crate::ResidentId) -> Option<&mut ResidentModel> {
        self.records.get_mut(id)?.model.as_mut()
    }

    pub fn model_or_insert_with(&mut self, id: crate::ResidentId, initialize: impl FnOnce() -> ResidentModel) -> &mut ResidentModel {
        self.entry(id).model.get_or_insert_with(initialize)
    }

    /// New canonical data wins when a file contains both layouts; orphan legacy
    /// IDs are retained until appearance data is available instead of being dropped.
    pub fn import_models(&mut self, legacy: std::collections::BTreeMap<crate::ResidentId, ResidentModel>) {
        for (id, model) in legacy { self.entry(id).model.get_or_insert(model); }
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
        assert_eq!(record.state.as_ref().unwrap(), &vec![7, 9]); assert_eq!(record.checkpoint_at, Some(1234));
        assert_eq!(loaded.records[&43].checkpoint_at, None);
    }

    #[test]
    fn legacy_model_import_preserves_orphans_and_canonical_precedence() {
        let mut residents = Residents::<Vec<u32>>::default();
        residents.seed(1, Some(4), Some(9), vec![5]);
        residents.model_or_insert_with(1, || ResidentModel { days: 12.0, ..Default::default() });
        let legacy = std::collections::BTreeMap::from([
            (1, ResidentModel { days: 2.0, ..Default::default() }),
            (2, ResidentModel { gone: true, job: Some(("Science".into(), 7)), ..Default::default() }),
        ]);
        residents.import_models(legacy);
        assert_eq!(residents.model(&1).unwrap().days, 12.0);
        assert!(residents.records[&2].state.is_none());
        let mut loaded: Residents<Vec<u32>> = serde_json::from_slice(&serde_json::to_vec(&residents).unwrap()).unwrap();
        loaded.seed(2, Some(6), Some(15), vec![8]);
        assert!(loaded.model(&2).unwrap().gone);
        assert_eq!(loaded.model(&2).unwrap().job, Some(("Science".into(), 7)));
        assert_eq!(loaded.records[&2].state.as_ref().unwrap(), &vec![8]);
    }

    #[test]
    fn previous_checkpoint_layout_reads_without_new_model_field() {
        let json = r#"{"records":{"42":{"household":7,"home":9,"state":[3,4],"checkpoint_at":12}}}"#;
        let loaded: Residents<Vec<u32>> = serde_json::from_str(json).unwrap();
        assert_eq!(loaded.records[&42].state.as_ref().unwrap(), &vec![3,4]);
        assert!(loaded.model(&42).is_none());
        let legacy: ResidentModel = serde_json::from_str(r#"{"age":"Adult","days":8.0,"job":["Business",4]}"#).unwrap();
        assert_eq!(legacy.days, 8.0);
        assert!(!legacy.dead && !legacy.gone);
    }

    #[test]
    fn survival_is_retained_across_model_roundtrip_and_legacy_span_is_unchanged() {
        let mut model = ResidentModel { days: 16.0, elder_risk: Some(0.8), ..Default::default() };
        assert!(!old_age_today(model.days, model.elder_span, &mut model.elder_risk, 0.3, 17.0));
        assert_eq!(model.elder_risk, Some(0.8));
        model.days = 17.0;
        for _ in 0..2 { assert!(!old_age_today(model.days, model.elder_span, &mut model.elder_risk, 0.3, 17.0)); }
        let mut restored: ResidentModel = serde_json::from_slice(&serde_json::to_vec(&model).unwrap()).unwrap();
        assert!(old_age_today(restored.days, restored.elder_span, &mut restored.elder_risk, 0.3, 17.0));
        assert!(old_age_today(18.0, 17.0, &mut None, 0.01, 17.0));
        for id in [0, 1, 42, u64::MAX] { assert!(initial_elder_risk(id).is_finite() && initial_elder_risk(id) > 0.0); }
    }

}
