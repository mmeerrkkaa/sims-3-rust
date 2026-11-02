//! Coarse resident ownership lives in TownStory, independently of model bodies.
use bevy::prelude::*;
use std::collections::HashSet;
use crate::{clock::GameClock,story::TownStory};
/// Keeps identity and personal components while the calendar owns movement.
#[derive(Component)]
pub(crate) struct CoarseActor;
#[derive(Resource)]
pub(crate) struct MobilityWorkload {pub last_reconcile:Option<s3sim::Tick>,pub processed:usize,pub overdue:usize,pub oldest_delay:s3sim::Tick,locations:std::collections::HashMap<String,usize>,school:Option<usize>,locations_ready:bool,pub park_after_seconds:f64,idle_since:std::collections::HashMap<Entity,f64>,pub parked:u64}
impl Default for MobilityWorkload {
    fn default()->Self {Self{last_reconcile:None,processed:0,overdue:0,oldest_delay:0,locations:default(),school:None,locations_ready:false,park_after_seconds:30.0,idle_since:default(),parked:0}}
}
/// Last actual lot is retained when old departure controllers clear OnLot.
#[derive(Component)]
pub(crate) struct OriginLot(pub u64);
/// Reconcile once per game minute, rather than scanning the whole city per tick.
/// A live detailed actor or dormant played household keeps its existing ownership.
pub(crate) fn advance(clock:Res<GameClock>,delta:Res<crate::clock::SimDelta>,world:Res<crate::loading::CurrentWorld>,
    actors:Query<(&crate::sim::Sim,Has<CoarseActor>)>,weather:Res<crate::weather::Weather>,dormant:Res<crate::household::Dormant>,mut story:ResMut<TownStory>,mut workload:ResMut<MobilityWorkload>) {
    if delta.0<=0.0 {return;}
    let now=s3sim::tick_at(clock.minutes);
    if workload.last_reconcile.is_none_or(|previous|now<previous || now/10!=previous/10) {
        workload.last_reconcile=Some(now);
        let detailed:HashSet<_>=actors.iter().filter(|(_,coarse)|!*coarse).map(|(sim,_)|sim.id).chain(dormant.member_ids()).collect();
        if !workload.locations_ready {
            for career in crate::careers::careers() {
                if let Some(lot)=(0..world.data.lots.len()).find(|lot|crate::careerholes::works_at(&world.data,*lot,career.key)) {workload.locations.insert(career.name.into(),lot);}
            }
            workload.school=(0..world.data.lots.len()).find(|lot|crate::opportunities::lot_types(&world.data,*lot).contains(&"School"));
            workload.locations_ready=true;
        }
        let holiday=crate::holidays::is_holiday(&weather,clock.day());
        let destinations:Vec<_>=world.data.lots.iter().enumerate().filter(|(index,_)|crate::visit::visitable(&world.data,*index)).map(|(_,lot)|lot).collect();
        let plans:Vec<_>=story.residents.records.iter().filter(|(id,record)|!detailed.contains(id) && record.model.as_ref().is_none_or(|model|model.present()))
            .filter_map(|(id,record)| {
                let state=record.state.as_ref()?;if crate::save::age_from_name(&state.age).is_little() || destinations.is_empty() {return None;}
                let home=record.home?;let home_lot=world.data.lots.iter().find(|lot|lot.id==home)?;
                let destination=destinations[(*id as usize)%destinations.len()];if destination.id==home {return None;}
                let distance=crate::home::lot_center(home_lot).distance(crate::home::lot_center(destination));
                let age=record.model.as_ref().and_then(|m|m.age.as_deref()).unwrap_or(&state.age);
                let school=if matches!(crate::save::age_from_name(age),crate::sim::Age::Child|crate::sim::Age::Teen) {
                    workload.school.map(|lot| {
                        let (start,end)=crate::rabbitholes::school_hours(crate::save::age_from_name(age));
                        s3sim::mobility::Commitment{purpose:s3sim::mobility::Purpose::School,destination:world.data.lots[lot].id,days:31,
                            start:s3sim::tick_at(start as f64*60.0),end:s3sim::tick_at(end as f64*60.0),travel:250,
                            cancelled_day:(holiday || weather.snow_day(clock.minutes)).then_some(clock.day() as u64)}
                    })
                } else {None};
                let career=record.model.as_ref().and_then(|m|m.job.as_ref());
                let work=career.and_then(|(name,level)| {
                    let lot=*workload.locations.get(name)?;let track=crate::careers::careers().iter().position(|c|c.name==name)?;
                    let mut job=crate::careers::Job::new(track);
                    if let Some(saved)=state.job.as_ref().filter(|j|j.track==*name) {job.branch=job.career().path_index(&saved.branch).unwrap_or(0);}
                    job.level=(*level as usize).saturating_sub(1).min(job.levels().len()-1);let info=job.info();
                    Some(s3sim::mobility::Commitment{purpose:s3sim::mobility::Purpose::Work,destination:world.data.lots[lot].id,
                        days:(0..7).filter(|day|job.works_on(*day)).fold(0,|mask,day|mask|(1<<day)),
                        start:s3sim::tick_at(info.start as f64*60.0),end:s3sim::tick_at(info.end as f64*60.0),
                        travel:((crate::home::lot_center(home_lot).distance(crate::home::lot_center(&world.data.lots[lot]))/12.0*10.0) as u64).clamp(50,300),
                        cancelled_day:holiday.then_some(clock.day() as u64)})
                });
                Some((*id,s3sim::mobility::Plan{home,destination:destination.id,departure:10_200+id%1200,
                    travel:((distance/12.0*10.0) as u64).clamp(50,300),stay:1200,work,school}))
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
    actors:Query<(Entity,&crate::sim::Sim,Has<crate::sim::HouseholdMember>,Has<crate::visit::OnLot>,Has<crate::visit::Trip>,Has<crate::interact::Invited>,Has<crate::interact::Visitor>,Has<crate::nav::PathFollow>,Has<CoarseActor>,&crate::interact::ActionQueue,Has<crate::meetups::Meeting>,Has<crate::careers::AtWork>,Has<crate::rabbitholes::AtRabbitHole>),With<crate::residents::PersonalStateReady>>) {
    for (entity,sim,member,on,trip,invited,visitor,path,coarse,queue,meeting,work,rabbit) in &actors {
        let busy=member || on || trip || invited || visitor || path || !queue.0.is_empty() || meeting || work || rabbit;
        let Some(pending)=story.mobility.residents.get(&sim.id) else {if busy && coarse {commands.entity(entity).remove::<CoarseActor>();}continue;};
        let at_home=matches!(pending.place,s3sim::mobility::Place::At{lot} if lot==pending.plan.home);
        // Rabbit-hole work/school and idle home records need logical identity,
        // not a public-lot visitor body. Preserve their original pending events.
        if !busy && (coarse || at_home || pending.activity.is_some()) {commands.entity(entity).insert(CoarseActor);continue;}
        let resident=story.mobility.take(sim.id).unwrap();commands.entity(entity).remove::<CoarseActor>();
        if busy {continue;}
        use s3sim::mobility::Place;
        let (lot,arrive,leave)=match resident.place {
            Place::At{lot} if lot==resident.plan.home=>continue,
            Place::At{lot}=>(lot,s3sim::tick_at(clock.minutes),resident.next),
            Place::Travel{to,arrive,..}=>(to,arrive,resident.release.unwrap_or_else(||arrive.saturating_add(resident.plan.stay))),
        };
        let Some(destination)=world.data.lots.iter().find(|l|l.id==lot) else {warn!("coarse handoff: missing lot {lot}");continue};
        let mut saved=crate::save::saved_look(sim);
        saved.world_location=Some(s3sim::residents::WorldLocation::ToLot{lot,arrive_at:arrive});
        saved.lot_role=Some(if lot==resident.plan.home {s3sim::residents::LotRole::Resident} else {s3sim::residents::LotRole::Guest{leave_at:leave}});
        saved.position=crate::home::lot_center(destination).to_array();
        crate::visit::restore_location(&mut commands.entity(entity),&world.data,&saved,clock.minutes);
    }
}

#[derive(bevy::ecs::query::QueryData)]
pub(crate) struct ParkState {
    entity:Entity,sim:&'static crate::sim::Sim,queue:&'static crate::interact::ActionQueue,
    off:Has<crate::interact::OffLot>,ready:Has<crate::residents::PersonalStateReady>,coarse:Has<CoarseActor>,
    origin:Option<&'static OriginLot>,visibility:&'static Visibility,
    member:Has<crate::sim::HouseholdMember>,selected:Has<crate::sim::Selected>,service:Has<crate::services::ServiceNpc>,ghost:Has<crate::ghosts::Ghost>,
    on:Has<crate::visit::OnLot>,trip:Has<crate::visit::Trip>,invited:Has<crate::interact::Invited>,visitor:Has<crate::interact::Visitor>,path:Has<crate::nav::PathFollow>,
    work:Has<crate::careers::AtWork>,rabbit:Has<crate::rabbitholes::AtRabbitHole>,meeting:Has<crate::meetups::Meeting>,staged:Has<crate::portraits::Staged>,
    social:Has<crate::interact::SocialPartner>,holding:Has<crate::carry::Holding>,carried:Has<crate::little::Carried>,
}
/// Personal data is checkpointed before a logical actor re-enters coarse control.
/// Keeping its Entity preserves the remaining Entity-based relationship/events.
pub(crate) fn park(mut commands:Commands,time:Res<Time<Real>>,clock:Res<GameClock>,world:Res<crate::loading::CurrentWorld>,
    actors:Query<ParkState>,capture:crate::save::CaptureResidents,mut story:ResMut<TownStory>,mut workload:ResMut<MobilityWorkload>,
    (used,seconds,privacy):(Query<&crate::interact::UsedBy>,Query<&crate::interact::SecondPlace>,Res<crate::privacy::Privacy>),
    (date,party):(Option<Res<crate::dates::Date>>,Option<Res<crate::parties::Party>>)) {
    let mut pinned:HashSet<Entity>=used.iter().filter_map(|o|o.0).chain(seconds.iter().filter_map(|o|o.0)).chain(privacy.objects.values().copied()).collect();
    if let Some(date)=date {pinned.extend([date.a,date.b]);}
    if let Some(party)=party {pinned.insert(party.host);pinned.extend(party.guests.iter().copied());for (a,b) in &party.couples {pinned.extend([*a,*b]);}}
    let alive:HashSet<_>=actors.iter().map(|s|s.entity).collect();workload.idle_since.retain(|e,_|alive.contains(e));
    let now=time.elapsed_secs_f64();let mut eligible=Vec::new();
    for state in &actors {
        let owned=story.residents.records.get(&state.sim.id).is_some_and(|r|r.home.is_some() && r.model.as_ref().is_none_or(|m|m.present()));
        let idle=owned && *state.visibility==Visibility::Hidden && state.off && state.ready && !state.coarse && state.queue.0.is_empty() && !pinned.contains(&state.entity)
            && !(state.member||state.selected||state.service||state.ghost||state.on||state.trip||state.invited||state.visitor||state.path||state.work||state.rabbit||state.meeting||state.staged||state.social||state.holding||state.carried);
        if !idle {workload.idle_since.remove(&state.entity);continue;}
        let since=*workload.idle_since.entry(state.entity).or_insert(now);
        if now-since>=workload.park_after_seconds {eligible.push(state.entity);}
    }
    eligible.sort_by_key(|e|e.to_bits());eligible.truncate(2);if eligible.is_empty() {return;}
    let wanted:HashSet<_>=eligible.iter().copied().collect();
    for mut saved in capture.capture_matching(|e|wanted.contains(&e)) {
        let Some(&entity)=eligible.iter().find(|e|actors.get(**e).is_ok_and(|state|state.sim.id==saved.id)) else {continue};
        let Ok(state)=actors.get(entity) else {continue};let home=story.residents.records[&saved.id].home.unwrap();
        let Some(home_lot)=world.data.lots.iter().find(|l|l.id==home) else {continue};
        let from=state.origin.map_or(home,|origin|origin.0);let origin=world.data.lots.iter().find(|l|l.id==from).unwrap_or(home_lot);
        let travel=((crate::home::lot_center(home_lot).distance(crate::home::lot_center(origin))/12.0*10.0) as u64).clamp(50,300);
        saved.background=None;saved.world_location=None;saved.lot_role=None;saved.whereabouts="away".into();
        crate::residents::checkpoint(&mut story,&mut saved,clock.minutes);
        // Normal reconciliation builds the full calendar on the next simulation
        // tick. Retain the physical return deadline immediately, even when paused.
        let destination=(0..world.data.lots.len()).find(|lot|crate::visit::visitable(&world.data,*lot));let Some(destination)=destination else {continue};
        let plan=s3sim::mobility::Plan{home,destination:world.data.lots[destination].id,departure:10200+saved.id%1200,travel,stay:1200,work:None,school:None};
        story.mobility.rejoin(saved.id,plan,s3sim::tick_at(clock.minutes),from,travel);
        commands.entity(entity).remove::<(crate::town::Townie,crate::homes::Resident,OriginLot)>().insert(CoarseActor);
        workload.idle_since.remove(&entity);workload.last_reconcile=None;workload.parked+=1;
    }
}
