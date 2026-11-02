//! Bounded cache for idle off-lot presentation. Logical entities and personal
//! components stay alive until the remaining Entity-based gameplay is migrated.
use std::collections::{HashMap,HashSet};
use bevy::prelude::*;
use bevy::ecs::query::QueryData;
use crate::{sim::{Sim,SimAnim,Pose},interact::ActionQueue};

#[derive(Component)]
pub(crate) struct UnloadedBody;

#[derive(Resource)]
pub(crate) struct BodyCache {
    pub max_idle_bodies:usize,
    pub idle_seconds:f64,
    idle_since:HashMap<Entity,f64>,
    requested:HashSet<Entity>,
    pub evicted:u64,
    pub woken:u64,
}
impl Default for BodyCache {
    fn default()->Self {Self{max_idle_bodies:12,idle_seconds:30.0,idle_since:default(),requested:default(),evicted:0,woken:0}}
}

impl BodyCache {pub(crate) fn demand(&mut self,entity:Entity) {self.requested.insert(entity);}}

#[derive(QueryData)]
pub(crate) struct BodyState {
    entity:Entity,
    sim:&'static Sim,
    visibility:&'static Visibility,
    queue:&'static ActionQueue,
    animation:&'static SimAnim,
    children:Option<&'static Children>,
    skeleton:Has<crate::simbody::Skeleton>,
    unloaded:Has<UnloadedBody>,
    ready:Has<crate::residents::PersonalStateReady>,
    member:Has<crate::sim::HouseholdMember>,
    selected:Has<crate::sim::Selected>,
    off_lot:Has<crate::interact::OffLot>,
    invited:Has<crate::interact::Invited>,
    visiting:Has<crate::interact::Visitor>,
    lot:Has<crate::visit::OnLot>,
    trip:Has<crate::visit::Trip>,
    path:Has<crate::nav::PathFollow>,
    work:Has<crate::careers::AtWork>,
    rabbit:Has<crate::rabbitholes::AtRabbitHole>,
    meeting:Has<crate::meetups::Meeting>,
    staged:Has<crate::portraits::Staged>,
    service:Has<crate::services::ServiceNpc>,
    ghost:Has<crate::ghosts::Ghost>,
    action_clip:Has<crate::anim::ActionClip>,
    holding:Has<crate::carry::Holding>,
    carried:Has<crate::little::Carried>,
    mounted:Has<crate::riding::Mounted>,
    overlay:Has<crate::seatedtalk::TalkOverlay>,
    props:Option<&'static crate::props::HeldProps>,
}
impl BodyStateItem<'_,'_> {
    fn idle(&self)->bool {
        self.ready && !self.member && !self.selected && self.off_lot && *self.visibility==Visibility::Hidden
            && self.queue.0.is_empty() && self.animation.pose==Pose::Stand
            && !(self.invited || self.visiting || self.lot || self.trip || self.path || self.work || self.rabbit
                || self.meeting || self.staged || self.service || self.ghost || self.action_clip
                || self.holding || self.carried || self.mounted || self.overlay)
            && self.props.is_none_or(|p|p.is_empty())
    }
}

/// Runs after Update gameplay. No camera distance is used: a route, selected
/// actor or queued action cannot be discarded by moving the camera elsewhere.
/// At most two bodies leave and two enter the cache per frame.
pub(crate) fn maintain(
    mut commands:Commands,time:Res<Time<Real>>,mut cache:ResMut<BodyCache>,
    actors:Query<BodyState>,parts:Query<(),With<crate::simbody::SimModelPart>>,
    portraits:Res<crate::portraits::Portraits>,capture:crate::save::CaptureResidents,
    mut story:ResMut<crate::story::TownStory>,clock:Res<crate::clock::GameClock>,
) {
    let now=time.elapsed_secs_f64();
    let alive:HashSet<_>=actors.iter().map(|s|s.entity).collect();
    cache.idle_since.retain(|e,_|alive.contains(e));
    cache.requested.retain(|e|alive.contains(e));
    let mut idle=Vec::new();
    let mut wake=Vec::new();
    for state in &actors {
        let needs_body=!state.idle() || portraits.needs_body(state.entity) || cache.requested.contains(&state.entity);
        if state.skeleton && !state.unloaded {cache.requested.remove(&state.entity);}
        if needs_body {
            cache.idle_since.remove(&state.entity);
            if state.unloaded {
                let priority=if state.selected {0} else if state.member {1} else if *state.visibility!=Visibility::Hidden {2} else if !state.idle() {3} else {4};
                wake.push((priority,state.entity));
            }
        } else if state.skeleton && !state.unloaded {
            let since=*cache.idle_since.entry(state.entity).or_insert(now);
            idle.push((state.entity,since));
        }
    }
    wake.sort_by_key(|(priority,e)|(*priority,e.to_bits()));
    for (_,e) in wake.into_iter().take(2) {
        cache.requested.remove(&e);
        commands.entity(e).remove::<UnloadedBody>().insert(crate::aging::NeedsNewBody);
        cache.woken+=1;
    }
    idle.sort_by(|a,b|a.1.total_cmp(&b.1).then_with(||a.0.to_bits().cmp(&b.0.to_bits())));
    let excess=idle.len().saturating_sub(cache.max_idle_bodies).min(2);
    let victims:HashSet<_>=idle.into_iter().filter(|(_,since)|now-*since>=cache.idle_seconds).take(excess).map(|(e,_)|e).collect();
    if victims.is_empty() {return;}
    let mut captured=HashSet::new();
    for mut state in capture.capture_matching(|e|victims.contains(&e)) {
        crate::residents::checkpoint(&mut story,&mut state,clock.minutes);
        captured.insert(state.id);
    }
    for e in victims {
        let Ok(state)=actors.get(e) else {continue};
        // If the shared rich capture could not see the actor, retain its body.
        if !captured.contains(&state.sim.id) {continue;}
        if let Some(children)=state.children {
            for child in children.iter().filter(|child|parts.contains(*child)) {commands.entity(child).try_despawn();}
        }
        commands.entity(e).remove::<crate::simbody::Skeleton>().insert(UnloadedBody);
        cache.idle_since.remove(&e);cache.evicted+=1;
    }
}
