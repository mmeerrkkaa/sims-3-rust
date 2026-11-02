//! Asking someone out by phone, as the game's `CallInviteToLot`: "Invite Out" to a community lot
//! anyone the caller knows (liking -20 and up, its `kLTRThresholdForInviteToLotMin`), or "Ask Out
//! on Date" someone romantic with them (both teens, or both young adults and up, romantically
//! involved or flirting: the game's `GroupingSituation.CanGoOnDate`). The caller heads there,
//! and whoever was asked turns up soon after they've arrived: on a date, the date begins
//! (`dates`); otherwise they come over and say hello. Nobody comes to meet them in six hours,
//! they've been stood up.

use bevy::prelude::*;

use crate::PlayMode;
use crate::clock::GameClock;
use crate::interact::{Action, ActionKind, ActionQueue, Notifications};
use crate::sim::{Age, Sim};
use crate::social::Relationships;

pub struct MeetupsPlugin;

impl Plugin for MeetupsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, resume_departures.run_if(in_state(PlayMode::Live)).in_set(crate::clock::SimPhase::Prepare).after(crate::residents::reconcile_daily))
            .add_systems(crate::clock::SimTick,turn_up.in_set(crate::careers::CommitmentPhase::Trips));
    }
}

/// The liking down to which someone may be asked out (`kLTRThresholdForInviteToLotMin`).
pub const INVITE_OUT_MIN: f32 = -20.0;
/// How long after the caller gets there the one asked turns up, and how long they stay
/// (game minutes); how long they wait to be met before giving up.
const TURN_UP_AFTER: f64 = 10.0;
const STAY: f64 = 240.0;
const GIVE_UP: f64 = 360.0;
/// How long the call takes (game minutes).
pub const CALL_MINUTES: f32 = 8.0;

/// Asked to meet someone at a lot: who, where, whether it's a date, and since when.
#[derive(Component)]
pub struct Meeting {
    pub with: u64,
    pub lot: usize,
    pub date: bool,
    pub since: f64,
    caller_there: Option<f64>,
}

impl Meeting {
    pub fn new(with: u64, lot: usize, date: bool, now: f64) -> Self {
        Self { with, lot, date, since: now, caller_there: None }
    }
}

/// A promise to meet uses durable world/resident identity, never scene handles.
#[derive(serde::Serialize,serde::Deserialize,Clone,Debug,PartialEq)]
pub struct SavedMeeting {
    pub with:u64,
    pub lot:u64,
    pub date:bool,
    pub since:f64,
    pub caller_there:Option<f64>,
}
impl Meeting {
    pub(crate) fn saved(&self,world:&crate::loading::WorldInfo)->Option<SavedMeeting> {
        Some(SavedMeeting{with:self.with,lot:world.lots.get(self.lot)?.id,date:self.date,since:self.since,caller_there:self.caller_there})
    }
    pub(crate) fn restore(saved:&SavedMeeting,world:&crate::loading::WorldInfo)->Option<Self> {
        Some(Self{with:saved.with,lot:world.lots.iter().position(|l|l.id==saved.lot)?,date:saved.date,since:saved.since,caller_there:saved.caller_there})
    }
}

/// Accepted outing awaiting the caller's departure. This promise survives a save
/// between finishing the phone call and the travel action starting.
#[derive(Component)]
pub(crate) struct OutingDeparture(pub usize);
fn resume_departures(mut commands:Commands,mut callers:Query<(Entity,&OutingDeparture,&mut ActionQueue,Option<&crate::visit::Trip>,Option<&crate::visit::OnLot>)>) {
    for (entity,departure,mut queue,trip,on) in &mut callers {
        if trip.is_some() || on.is_some_and(|lot|lot.0==departure.0) {commands.entity(entity).remove::<OutingDeparture>();continue;}
        if queue.0.iter().any(|action|action.cancel && matches!(action.kind,ActionKind::GoToLot{lot} if lot==departure.0)) {
            commands.entity(entity).remove::<OutingDeparture>();continue;
        }
        if !queue.0.iter().any(|action|matches!(action.kind,ActionKind::GoToLot{lot} if lot==departure.0)) {
            queue.0.push_back(Action::new("Visit Lot",ActionKind::GoToLot{lot:departure.0},false));
        }
    }
}

/// Whether the two could go on a date (the game's `CanGoOnDate`): both teens, or both young
/// adults and up; and romantically involved, or their talk gone romantic.
pub fn can_date(a: &Sim, b: &Sim, rels: &Relationships, other: Entity) -> bool {
    rels.0.get(&other).is_some_and(|r|can_date_relationship(a,b,r))
}

pub(crate) fn can_date_relationship(a:&Sim,b:&Sim,r:&crate::social::Relationship)->bool {
    let grown = |s: &Sim| s.age.is_grown() && s.age != Age::Child;
    let ages = (a.age == Age::Teen && b.age == Age::Teen) || (grown(a) && grown(b) && a.age != Age::Teen && b.age != Age::Teen);
    use crate::social::RelStatus;
    let together = |s: RelStatus| matches!(s, RelStatus::Partner | RelStatus::Engaged | RelStatus::Married);
    let romantic = r.romantic || together(r.status) || (r.stc.commodity == crate::conversation::Commodity::Amorous && r.stc.value > 0.0);
    ages && romantic
}

/// Whoever was asked out turns up once the caller's there (out of sight till then); stood up
/// after six hours.
#[allow(clippy::type_complexity)]
fn turn_up(
    mut commands: Commands,
    clock: Res<GameClock>,
    world: Res<crate::loading::CurrentWorld>,
    building: Option<Res<crate::building::ActiveBuilding>>,
    visited: Res<crate::visit::OpenLots>,
    mut asked: Query<(Entity, &Sim, &mut Meeting, &mut Transform, Option<&mut crate::nav::Floor>)>,
    callers: Query<(&Sim, Option<&crate::visit::OnLot>), Without<Meeting>>,
    (actors,story,mut requests):(Res<crate::residents::ResidentActors>,Res<crate::story::TownStory>,ResMut<crate::residents::MaterializeRequests>),
    mut starts: MessageWriter<crate::dates::StartDate>,
    mut notes: ResMut<Notifications>,
) {
    let now = clock.minutes;
    let mut rng = rand::rng();
    for (e, sim, mut m, mut tf, floor) in &mut asked {
        // A caller without a scene entity still owns the promise. Do not
        // cancel a loaded meeting merely because its caller materializes later.
        if !story.model(&m.with).is_none_or(|model|model.present()) {
            commands.entity(e).remove::<Meeting>();continue;
        }
        let caller=actors.by_id.get(&m.with).and_then(|entity|callers.get(*entity).ok().map(|row|(*entity,row)));
        let Some((caller_entity,(caller,on)))=caller else {
            if now-m.since>GIVE_UP {commands.entity(e).remove::<Meeting>();}
            else if story.residents.records.get(&m.with).is_some_and(|r|r.state.is_some()) {requests.request(m.with);}
            continue;
        };
        let here = on.is_some_and(|o| o.0 == m.lot) && visited.0.get(&m.lot).is_some_and(|v| !v.grid.dirty);
        if here && m.caller_there.is_none() {
            m.caller_there = Some(now);
        }
        if let (Some(t), Some(v)) = (m.caller_there, visited.0.get(&m.lot))
            && here
            && now >= t + TURN_UP_AFTER
        {
            use rand::Rng;
            let p = v.exit + Vec2::new(rng.random_range(-1.5..1.5), rng.random_range(-1.0..1.0));
            tf.translation = Vec3::new(p.x, crate::building::walk_height(&world.data, building.as_deref(), Vec3::new(p.x, 0.0, p.y)), p.y);
            if let Some(mut f) = floor {
                f.0 = 1;
            }
            commands
                .entity(e)
                .remove::<(Meeting, crate::interact::OffLot, crate::interact::Invited, crate::town::Townie, crate::nav::PathFollow)>()
                .insert((crate::visit::LotGuest { leave_at: now + STAY }, crate::visit::OnLot(m.lot), Visibility::Inherited))
                .insert_if_new((ActionQueue::default(), crate::interact::AutonomyTimer(2.0), crate::interact::Skills::default()));
            let with = caller_entity;
            if m.date {
                starts.write(crate::dates::StartDate { a: with, b: e });
            } else {
                notes.push(format!("{} has come to meet {}.", sim.first, caller.first));
                if let Some(si) = crate::social::social_index("Greet") {
                    commands.entity(e).queue_silenced(move |mut w: EntityWorldMut| {
                        if let Some(mut q) = w.get_mut::<ActionQueue>() {
                            q.0.push_back(Action::new("Greet", ActionKind::Social { target: with, social: si }, true));
                        }
                    });
                }
            }
            continue;
        }
        if now - m.since > GIVE_UP {
            notes.push(format!("{} waited for {} and gave up.", sim.first, caller.first));
            commands.entity(e).remove::<Meeting>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_for_the_romantic_of_an_age() {
        let mut rng = rand::rng();
        let a = crate::sim::random_sim(&mut rng, "A", Some(true), Age::Adult);
        let mut b = crate::sim::random_sim(&mut rng, "B", Some(false), Age::YoungAdult);
        let other = Entity::from_raw_u32(7).unwrap();
        let mut rels = Relationships::default();
        assert!(!can_date(&a, &b, &rels, other), "strangers");
        rels.entry(other).status = crate::social::RelStatus::Married;
        assert!(can_date(&a, &b, &rels, other), "married");
        rels.entry(other).status = crate::social::RelStatus::Ex;
        assert!(!can_date(&a, &b, &rels, other), "an ex");
        rels.entry(other).romantic = true;
        assert!(can_date(&a, &b, &rels, other));
        b.age = Age::Teen;
        assert!(!can_date(&a, &b, &rels, other), "a grown-up and a teen");
    }
}

#[cfg(local_validation)]
pub(crate) fn validate_turn_up(world:&mut World) {use bevy::ecs::system::RunSystemOnce;world.run_system_once(turn_up).unwrap();}

#[cfg(local_validation)]
pub(crate) fn validate_departures(world:&mut World) {use bevy::ecs::system::RunSystemOnce;world.run_system_once(resume_departures).unwrap();}
