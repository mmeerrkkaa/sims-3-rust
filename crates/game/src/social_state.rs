//! Persistent social situations bind resident identities after scene restoration.
use bevy::prelude::*;
use serde::{Serialize,Deserialize};
use std::collections::{HashMap,BTreeMap};

#[derive(Serialize,Deserialize,Clone,Debug,Default,PartialEq)]
pub struct SavedEvents {pub date:Option<SavedDate>,pub party:Option<SavedParty>}
#[derive(Serialize,Deserialize,Clone,Debug,PartialEq)]
pub struct SavedDate {pub a:u64,pub b:u64,pub until:f64,pub start:f32,pub next_nudge:f64}
#[derive(Serialize,Deserialize,Clone,Debug,PartialEq)]
pub struct SavedParty {
    pub host:u64,pub kind:crate::parties::PartyKind,pub dress:crate::parties::Dress,pub start:f64,
    pub guests:Vec<u64>,pub here:BTreeMap<u64,f64>,pub gone:Vec<u64>,pub left:u32,pub moods:f32,
    pub begun:bool,pub told:bool,pub succeeded:bool,pub couples:Vec<(u64,u64)>,pub wedding_day:bool,pub vows:bool,pub last_check:f64,
}
#[derive(Resource)]
pub(crate) struct PendingEvents(pub SavedEvents);
impl SavedEvents {
    pub(crate) fn capture(date:Option<&crate::dates::Date>,party:Option<&crate::parties::Party>,ids:&HashMap<Entity,u64>)->Self {
        let date=date.and_then(|d|Some(SavedDate{a:*ids.get(&d.a)?,b:*ids.get(&d.b)?,until:d.until,start:d.start,next_nudge:d.next_nudge}));
        let party=party.and_then(|p| {
            let mut guests:Vec<_>=p.guests.iter().filter_map(|e|ids.get(e).copied()).collect();
            for id in &p.gone {if !guests.contains(id) {guests.push(*id);}}
            Some(SavedParty{host:*ids.get(&p.host)?,kind:p.kind,dress:p.dress,start:p.start,guests,
                here:p.here.iter().filter_map(|(e,t)|Some((*ids.get(e)?,*t))).collect(),gone:p.gone.clone(),left:p.left,moods:p.moods,
                begun:p.begun,told:p.told,succeeded:p.succeeded,couples:p.couples.iter().filter_map(|(a,b)|Some((*ids.get(a)?,*ids.get(b)?))).collect(),
                wedding_day:p.wedding_day,vows:p.vows,last_check:p.last_check})
        });
        Self{date,party}
    }
    pub(crate) fn participants(&self)->Vec<u64> {
        let mut ids=Vec::new();if let Some(d)=&self.date {ids.extend([d.a,d.b]);}
        if let Some(p)=&self.party {ids.push(p.host);ids.extend(p.guests.iter().filter(|id|!p.gone.contains(id)));ids.extend(p.here.keys());for (a,b) in &p.couples {ids.extend([*a,*b]);}}
        ids.sort_unstable();ids.dedup();ids
    }
}
pub(crate) fn clear(mut commands:Commands,mut picker:ResMut<crate::parties::PartyPicker>) {
    commands.remove_resource::<crate::dates::Date>();commands.remove_resource::<crate::parties::Party>();commands.remove_resource::<PendingEvents>();*picker=default();
}
pub(crate) fn restore(mut commands:Commands,pending:Option<Res<PendingEvents>>,index:Res<crate::residents::ResidentActors>,
    story:Res<crate::story::TownStory>,mut requests:ResMut<crate::residents::MaterializeRequests>,
    ready:Query<(),Or<(With<crate::residents::PersonalStateReady>,With<crate::ghosts::Ghost>,With<crate::services::ServiceNpc>)>>,mut visitors:Query<&mut crate::interact::Visitor>) {
    let Some(pending)=pending else {return};
    let available=|id:&u64| story.model(id).is_none_or(|m|m.present());
    let mut waiting=false;
    for id in pending.0.participants().into_iter().filter(available) {
        if let Some(e)=index.by_id.get(&id) {if !ready.contains(*e) {waiting=true;}}
        else if story.residents.records.get(&id).is_some_and(|r|r.state.is_some()) {requests.request(id);waiting=true;}
    }
    if waiting {return;}
    let entity=|id:&u64|available(id).then(||index.by_id.get(id).copied()).flatten();
    if let Some(d)=&pending.0.date {
        if let (Some(a),Some(b))=(entity(&d.a),entity(&d.b)) {commands.insert_resource(crate::dates::Date{a,b,until:d.until,start:d.start,next_nudge:d.next_nudge});}
        else {warn!("cannot restore date: participant unavailable");}
    }
    if let Some(p)=&pending.0.party {
        if let Some(host)=entity(&p.host) {
            let here:HashMap<_,_>=p.here.iter().filter_map(|(id,t)|Some((entity(id)?,*t))).collect();
            for e in here.keys() {if let Ok(mut visitor)=visitors.get_mut(*e) {visitor.leave_at=f64::INFINITY;}}
            commands.insert_resource(crate::parties::Party{host,kind:p.kind,dress:p.dress,start:p.start,
                guests:p.guests.iter().filter(|id|!p.gone.contains(id)).filter_map(entity).collect(),here,gone:p.gone.clone(),left:p.left,moods:p.moods,
                begun:p.begun,told:p.told,succeeded:p.succeeded,couples:p.couples.iter().filter_map(|(a,b)|Some((entity(a)?,entity(b)?))).collect(),
                wedding_day:p.wedding_day,vows:p.vows,last_check:p.last_check});
        } else {warn!("cannot restore party: host unavailable");}
    }
    commands.remove_resource::<PendingEvents>();
}
