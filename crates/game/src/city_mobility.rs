//! Coarse resident ownership lives in TownStory, independently of model bodies.
use bevy::prelude::*;
use std::collections::HashSet;
use crate::{clock::GameClock,story::TownStory};
#[derive(Resource,Default)]
pub(crate) struct MobilityWorkload {pub last_reconcile:Option<s3sim::Tick>,pub processed:usize,pub overdue:usize,pub oldest_delay:s3sim::Tick}
/// Reconcile once per game minute, rather than scanning the whole city per tick.
/// A live detailed actor or dormant played household keeps its existing ownership.
pub(crate) fn advance(clock:Res<GameClock>,delta:Res<crate::clock::SimDelta>,world:Res<crate::loading::CurrentWorld>,
    actors:Query<&crate::sim::Sim>,dormant:Res<crate::household::Dormant>,mut story:ResMut<TownStory>,mut workload:ResMut<MobilityWorkload>) {
    if delta.0<=0.0 {return;}
    let now=s3sim::tick_at(clock.minutes);
    if workload.last_reconcile.is_none_or(|previous|now<previous || now/10!=previous/10) {
        workload.last_reconcile=Some(now);
        let detailed:HashSet<_>=actors.iter().map(|sim|sim.id).chain(dormant.member_ids()).collect();
        let destinations:Vec<_>=world.data.lots.iter().enumerate().filter(|(index,_)|crate::visit::visitable(&world.data,*index)).map(|(_,lot)|lot).collect();
        let plans:Vec<_>=story.residents.records.iter().filter(|(id,record)|!detailed.contains(id) && record.model.as_ref().is_none_or(|model|model.present()))
            .filter_map(|(id,record)| {
                let state=record.state.as_ref()?;if crate::save::age_from_name(&state.age).is_little() || destinations.is_empty() {return None;}
                let home=record.home?;let home_lot=world.data.lots.iter().find(|lot|lot.id==home)?;
                let destination=destinations[(*id as usize)%destinations.len()];if destination.id==home {return None;}
                let distance=crate::home::lot_center(home_lot).distance(crate::home::lot_center(destination));
                Some((*id,s3sim::mobility::Plan{home,destination:destination.id,departure:10_200+id%1200,
                    travel:((distance/12.0*10.0) as u64).clamp(50,300),stay:1200}))
            }).collect();
        let roster:HashSet<_>=plans.iter().map(|(id,_)|*id).collect();
        story.mobility.retain(|id|roster.contains(&id));
        for (id,plan) in plans {story.mobility.seed(id,plan,now);}
    }
    let work=story.mobility.advance(now,64);workload.processed+=work.processed;workload.overdue=work.overdue;workload.oldest_delay=work.oldest_delay;
}
pub(crate) fn reset_frame(mut workload:ResMut<MobilityWorkload>) {workload.processed=0;}

/// Newly detailed actors receive the same trip deadline and destination. Existing
/// detailed activity (including an explicit invitation) outranks coarse planning.
pub(crate) fn handoff(mut commands:Commands,clock:Res<GameClock>,world:Res<crate::loading::CurrentWorld>,mut story:ResMut<TownStory>,
    actors:Query<(Entity,&crate::sim::Sim,Has<crate::sim::HouseholdMember>,Has<crate::visit::OnLot>,Has<crate::visit::Trip>,Has<crate::interact::Invited>,Has<crate::interact::Visitor>,Has<crate::nav::PathFollow>),With<crate::residents::PersonalStateReady>>) {
    for (entity,sim,member,on,trip,invited,visitor,path) in &actors {
        let Some(resident)=story.mobility.take(sim.id) else {continue};
        if member || on || trip || invited || visitor || path {continue;}
        use s3sim::mobility::Place;
        let (lot,arrive,leave)=match resident.place {
            Place::At{lot} if lot==resident.plan.home=>continue,
            Place::At{lot}=>(lot,s3sim::tick_at(clock.minutes),resident.next),
            Place::Travel{to,arrive,..}=>(to,arrive,arrive.saturating_add(resident.plan.stay)),
        };
        let Some(destination)=world.data.lots.iter().find(|l|l.id==lot) else {warn!("coarse handoff: missing lot {lot}");continue};
        let mut saved=crate::save::saved_look(sim);
        saved.world_location=Some(s3sim::residents::WorldLocation::ToLot{lot,arrive_at:arrive});
        saved.lot_role=Some(if lot==resident.plan.home {s3sim::residents::LotRole::Resident} else {s3sim::residents::LotRole::Guest{leave_at:leave}});
        saved.position=crate::home::lot_center(destination).to_array();
        crate::visit::restore_location(&mut commands.entity(entity),&world.data,&saved,clock.minutes);
    }
}
