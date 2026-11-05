//! Coarse minute integration shares natural decay and interaction rates with
//! detailed gameplay. Furniture availability gates abstract self-care.
use bevy::prelude::*;
use serde::{Serialize,Deserialize};
use std::collections::HashMap;
use crate::sim::*;
#[derive(Component,Clone,Copy,Debug,PartialEq,Serialize,Deserialize)]
pub struct NeedClock {pub at:s3sim::Tick,pub sleeping:bool}
#[derive(Clone,Copy,Default)]
pub(crate) struct Care {pub bed:bool,pub food:bool,pub toilet:bool,pub shower:bool}
#[derive(Resource,Default)]
pub(crate) struct CareLots(pub HashMap<u64,Care>);
pub(crate) fn prepare(world:Res<crate::loading::CurrentWorld>,mut lots:ResMut<CareLots>) {
    if !lots.0.is_empty() && !world.is_changed() {return;}
    lots.0.clear();
    for (index,building) in &world.data.buildings {
        let Some(lot)=world.data.lots.get(*index) else {continue};let mut care=Care::default();
        for object in &building.objects {
            let script=&object.script;
            care.bed|=script.contains("Bed");care.food|=script.contains("Fridge");care.toilet|=script.contains("Toilet");
            care.shower|=script.contains("Shower") || script.contains("Bathtub");
        }
        lots.0.insert(lot.id,care);
    }
}
fn action(kind:crate::interact::ObjectKind,name:&str)->&'static crate::interact::InteractionDef {
    crate::interact::interactions_for(kind).iter().find(|a|a.name==name).expect("shared coarse interaction")
}
#[cfg(any(test,local_validation))]
pub(crate) fn integrate(saved:&mut crate::save::SavedSim,resident:&s3sim::mobility::Resident,until:s3sim::Tick,care:Care,exact:bool) {
    let mut left=usize::MAX;integrate_budgeted(saved,resident,until,care,exact,&mut left);
}
/// Stop only on the same global minute boundaries used by an uninterrupted run.
/// The saved cursor and sleeping flag are sufficient to resume the exact state.
pub(crate) fn integrate_budgeted(saved:&mut crate::save::SavedSim,resident:&s3sim::mobility::Resident,until:s3sim::Tick,care:Care,exact:bool,left:&mut usize)->bool {
    let Some(mut clock)=saved.coarse_needs else {saved.coarse_needs=Some(NeedClock{at:until,sleeping:false});return true};
    if until<=clock.at {return true;}
    let end=if exact {until}else{until/10*10};if end<=clock.at {return true;}
    if *left==0 {return false;}
    let freeze_decay=crate::cheats::no_decay();
    let age=crate::save::age_from_name(&saved.age);let traits:Vec<_>=saved.traits.iter().filter_map(|name|crate::life::Trait::from_name(name)).collect();
    let reward=|name:&str|saved.rewards.iter().any(|r|r==name);
    let has_steel=reward("SteelBladder");let decay:[f32;6]=std::array::from_fn(|i|crate::wishes::reward_decay_with(i,reward));
    let at_home=matches!(resident.place,s3sim::mobility::Place::At{lot} if lot==resident.plan.home);
    let stationary=matches!(resident.place,s3sim::mobility::Place::At{..});
    let in_class=matches!(resident.place,s3sim::mobility::Place::At{lot} if resident.activity.is_some_and(|duty|lot==duty.destination && lot!=resident.plan.home));
    let mut motives=Motives(saved.motives);
    while clock.at<end && *left>0 {
        *left-=1;
        let next=((clock.at/10+1)*10).min(end);let hours=(next-clock.at) as f32/600.0;
        let hour=(clock.at%s3sim::mobility::DAY) as f32/600.0;
        if !at_home || !care.bed || motives.0[ENERGY]>=99.0 || (6.0..22.0).contains(&hour) && motives.0[ENERGY]>0.0 {clock.sleeping=false;}
        if at_home && care.bed && crate::interact::may_sleep(motives.0[ENERGY],crate::interact::hours_until_waking(hour,None)) {clock.sleeping=true;}
        let selected=if in_class {None} else if clock.sleeping {Some(action(crate::interact::ObjectKind::BedSingle,"Sleep"))}
            else if stationary && care.toilet && motives.0[BLADDER]<0.0 {Some(action(crate::interact::ObjectKind::Toilet,"Use Toilet"))}
            else if stationary && care.food && motives.0[HUNGER]<20.0 {Some(action(crate::interact::ObjectKind::Fridge,"Grab a Snack"))}
            else if stationary && care.shower && motives.0[HYGIENE]<0.0 {Some(action(crate::interact::ObjectKind::Shower,"Take Shower"))}
            else {None};
        let stuffed=saved.moodlets.iter().any(|(name,until)|name=="Stuffed" && *until>clock.at as f64/10.0);
        if stuffed {motives.0[HUNGER]=100.0;}if has_steel {motives.0[BLADDER]=100.0;}
        for i in 0..6 {
            if (stuffed && i==HUNGER) || (has_steel && i==BLADDER) {continue;}
            if in_class && (i==HUNGER || i==BLADDER) {continue;}
            let scale=selected.map_or(1.0,|a|a.decay[i]);
            let loss=crate::sim::natural_decay(i,motives.0[i],age,&traits)*crate::life::decay_rate(&traits,i)*decay[i]*scale*hours*if freeze_decay {0.0}else{1.0};
            let loss=if i==FUN {loss.min(motives.0[i].max(0.0))}else{loss};motives.add(i,-loss);
        }
        if let Some(selected)=selected {for i in 0..6 {motives.add(i,selected.per_hour[i]*hours);}}
        // At work/school the existing place supplies lunch and bathroom access.
        if in_class && resident.activity.is_some_and(|a|a.purpose==s3sim::mobility::Purpose::School) {
            for i in 0..6 {motives.add(i,crate::rabbitholes::SCHOOL.per_hour[i]*hours);}
        }

        clock.at=next;
    }
    saved.motives=motives.0;saved.coarse_needs=Some(clock);clock.at>=end
}
/// Shared quota covers event accounting, periodic care and automatic handoff.
/// Repeated calls at the same game tick do not replenish it.
#[derive(Resource)]
pub(crate) struct NeedWork {
    pub limit:usize,
    tick:Option<s3sim::Tick>,spent:usize,visited:usize,
    round:Option<(s3sim::Tick,Option<u64>)>,last_round:Option<s3sim::Tick>,
    pub frame_steps:usize,pub frame_visits:usize,
}
impl Default for NeedWork {
    fn default()->Self {Self{limit:2048,tick:None,spent:0,visited:0,round:None,last_round:None,frame_steps:0,frame_visits:0}}
}
impl NeedWork {
    fn begin(&mut self,now:s3sim::Tick) {
        if self.tick!=Some(now) {
            if self.tick.is_some_and(|old|now<old) {self.round=None;self.last_round=None;}
            self.tick=Some(now);self.spent=0;self.visited=0;
        }
    }
    pub(crate) fn account(&mut self,now:s3sim::Tick,saved:&mut crate::save::SavedSim,resident:&s3sim::mobility::Resident,until:s3sim::Tick,care:Care,exact:bool)->bool {
        self.begin(now);let available=self.limit.saturating_sub(self.spent);let mut left=available;
        let ready=integrate_budgeted(saved,resident,until,care,exact,&mut left);
        self.spent+=available-left;self.frame_steps+=available-left;ready
    }
    pub(crate) fn periodic(&mut self,now:s3sim::Tick,city:&s3sim::mobility::City,residents:&mut s3sim::residents::Residents<crate::save::SavedSim>,care:&CareLots) {
        self.begin(now);let minute=now/10*10;
        if self.round.is_none() && self.last_round!=Some(minute) {self.round=Some((minute,None));}
        while self.visited<256 && self.spent<self.limit {
            let Some((at,after))=self.round else {break};
            let bound=after.map_or(std::ops::Bound::Unbounded,std::ops::Bound::Excluded);
            let Some((&id,resident))=city.residents.range((bound,std::ops::Bound::Unbounded)).next() else {self.round=None;self.last_round=Some(at);break};
            self.visited+=1;self.frame_visits+=1;
            let ready=if let Some(saved)=residents.records.get_mut(&id).and_then(|r|r.state.as_mut()) {
                let lot=match resident.place {s3sim::mobility::Place::At{lot}=>Some(lot),_=>None};
                self.account(now,saved,resident,at.min(resident.next),lot.and_then(|l|care.0.get(&l).copied()).unwrap_or_default(),false)
            } else {true};
            if !ready {break;}
            self.round=Some((at,Some(id)));
        }
    }
}

/// Mirror only actors still owned by the coarse calendar. A detailed actor keeps
/// its current values; entering/leaving coarse ownership uses full checkpoints.
pub(crate) fn mirror(mut commands:Commands,story:Res<crate::story::TownStory>,mut actors:Query<(Entity,&Sim,&mut Motives),With<crate::city_mobility::CoarseActor>>) {
    for (entity,sim,mut motives) in &mut actors {
        if let Some(saved)=story.residents.records.get(&sim.id).and_then(|r|r.state.as_ref()) {
            motives.0=saved.motives;if let Some(clock)=saved.coarse_needs {commands.entity(entity).insert(clock);}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_minute_cursor_is_chunk_independent_and_furniture_gates_care() {
        let mut saved=crate::save::saved_sim_for_test(7,"Needs");saved.traits.clear();saved.motives=[10.0,-20.0,-70.0,30.0,-20.0,0.0];
        saved.coarse_needs=Some(NeedClock{at:13200,sleeping:false});
        let resident=s3sim::mobility::Resident{plan:s3sim::mobility::Plan{home:1,destination:2,departure:10200,travel:200,stay:1200,work:None,school:None},place:s3sim::mobility::Place::At{lot:1},next:24600,transitions:0,activity:None,release:None};
        let mut split=saved.clone();let care=Care{bed:true,food:true,toilet:true,shower:true};
        integrate(&mut saved,&resident,14400,care,false);
        for at in (13210..=13800).step_by(10) {integrate(&mut split,&resident,at,care,false);}
        let mut split:crate::save::SavedSim=serde_json::from_str(&serde_json::to_string(&split).unwrap()).unwrap();
        for at in (13810..=14400).step_by(10) {integrate(&mut split,&resident,at,care,false);}
        assert_eq!(split.motives,saved.motives);assert_eq!(split.coarse_needs,saved.coarse_needs);assert!(saved.motives[ENERGY]> -70.0);
        let mut empty=saved.clone();empty.motives=[10.0,-20.0,-70.0,30.0,-20.0,0.0];empty.coarse_needs=Some(NeedClock{at:13200,sleeping:false});
        integrate(&mut empty,&resident,14400,Care::default(),false);assert!(empty.motives[ENERGY]< -70.0);assert!(empty.motives[HUNGER]<10.0);
        let before=split.motives;integrate(&mut split,&resident,14400,care,false);assert_eq!(before,split.motives);
    }
    #[test]
    fn quota_preserves_partial_cursor_and_sleep_through_save() {
        let mut saved=crate::save::saved_sim_for_test(7,"Budget");saved.traits.clear();saved.motives=[10.0,-20.0,-70.0,30.0,-20.0,0.0];saved.coarse_needs=Some(NeedClock{at:13200,sleeping:false});
        let resident=s3sim::mobility::Resident{plan:s3sim::mobility::Plan{home:1,destination:2,departure:10200,travel:200,stay:1200,work:None,school:None},place:s3sim::mobility::Place::At{lot:1},next:24600,transitions:0,activity:None,release:None};
        let care=Care{bed:true,food:true,toilet:true,shower:true};let mut expected=saved.clone();integrate(&mut expected,&resident,14403,care,true);
        let mut work=NeedWork{limit:8,..default()};assert!(!work.account(14403,&mut saved,&resident,14403,care,true));assert_eq!(saved.coarse_needs.unwrap().at,13280);assert!(saved.coarse_needs.unwrap().sleeping);
        let partial=saved.motives;assert!(!work.account(14403,&mut saved,&resident,14403,care,true));assert_eq!(saved.motives,partial);assert_eq!(work.frame_steps,8);
        let mut saved:crate::save::SavedSim=serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();let mut work=NeedWork{limit:8,..default()};
        for tick in 14404..14500 {if work.account(tick,&mut saved,&resident,14403,care,true) {break;}}
        assert_eq!(saved.coarse_needs,expected.coarse_needs);assert_eq!(saved.motives,expected.motives);
    }

}
