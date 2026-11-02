//! Durable coarse journeys between world lots. No scene entities or rendering.
//! This is an event queue, not a coalescible reconsideration agenda: overdue
//! departures and arrivals are consumed in order, with their original dates.
use std::collections::{BTreeMap,BTreeSet};
use serde::{Serialize,Deserialize};
use crate::{ResidentId,Tick,residents::LotId};
pub const DAY:Tick=14_400;
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Plan {
    pub home:LotId,pub destination:LotId,pub departure:Tick,pub travel:Tick,pub stay:Tick,
    #[serde(default)] pub work:Option<Commitment>,
    #[serde(default)] pub school:Option<Commitment>,
}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub enum Purpose {Work,School}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Commitment {
    pub purpose:Purpose,pub destination:LotId,pub days:u8,pub start:Tick,pub end:Tick,pub travel:Tick,
    #[serde(default)] pub cancelled_day:Option<u64>,
}
impl Commitment {
    fn occurrence(self,day:u64)->Option<(Tick,Tick)> {
        if self.days&(1<<(day%7))==0 || self.cancelled_day==Some(day) {return None;}
        let start=day*DAY+self.start;let end=day*DAY+self.end+if self.end<=self.start {DAY} else {0};
        Some((start.saturating_sub(self.travel),end))
    }
}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub enum Place {At{lot:LotId},Travel{from:LotId,to:LotId,depart:Tick,arrive:Tick}}
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Resident {
    pub plan:Plan,pub place:Place,pub next:Tick,pub transitions:u64,
    #[serde(default)] pub activity:Option<Commitment>,
    #[serde(default)] pub release:Option<Tick>,
}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct City {
    #[serde(default)] completed:std::collections::VecDeque<Completed>,
    pub residents:BTreeMap<ResidentId,Resident>,
    due:BTreeSet<(Tick,ResidentId)>,
    budget_tick:Option<Tick>,spent:usize,
}
/// A completed commitment remains pending until its personal state owner applies
/// it. Saving between dispatch and application cannot lose attendance/results.
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Completed {pub resident:ResidentId,pub purpose:Purpose,pub started:Tick,pub ended:Tick}
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub struct Work {pub processed:usize,pub overdue:usize,pub oldest_delay:Tick}
impl Plan {
    fn validate(self) {assert!(self.departure<DAY);assert!(self.travel>0 && self.stay>0);assert_ne!(self.home,self.destination);}
    fn next_visit(self,after:Tick)->(Tick,Option<Commitment>,Option<Tick>) {
        let first=(after/DAY).saturating_sub(1);let mut best=(Tick::MAX,None,None);
        for day in first..=first+9 {
            for activity in [self.work,self.school].into_iter().flatten() {
                if let Some((depart,end))=activity.occurrence(day) {
                    if depart>after && depart<best.0 {best=(depart,Some(activity),Some(end));}
                }
            }
            let leisure=day*DAY+self.departure;
            let end=leisure.saturating_add(self.travel*2).saturating_add(self.stay);
            let overlaps=[self.work,self.school].into_iter().flatten().any(|duty|
                (day.saturating_sub(1)..=day+1).any(|d|duty.occurrence(d).is_some_and(|(from,until)|leisure<until.saturating_add(duty.travel) && end>from)));
            if leisure>after && leisure<best.0 && !overlaps {best=(leisure,None,None);}
        }
        assert_ne!(best.0,Tick::MAX,"calendar must have a future event");best
    }
}
impl City {
    pub fn pop_completed(&mut self)->Option<Completed> {self.completed.pop_front()}
    pub fn completed_count(&self)->usize {self.completed.len()}

    /// Reconcile only cold residents. A detailed actor's state must be captured
    /// before it enters this roster; removing it transfers ownership out of here.
    pub fn retain(&mut self,mut keep:impl FnMut(ResidentId)->bool) {
        self.residents.retain(|id,_|keep(*id));self.due.retain(|(_,id)|self.residents.contains_key(id));
    }
    /// Transfer the full pending journey to detailed ownership atomically.
    pub fn take(&mut self,id:ResidentId)->Option<Resident> {
        let resident=self.residents.remove(&id)?;self.due.remove(&(resident.next,id));Some(resident)
    }
    /// A retired detailed visitor still has a real return journey to its home.
    pub fn rejoin(&mut self,id:ResidentId,plan:Plan,now:Tick,from:LotId,travel:Tick) {
        self.take(id);self.seed(id,plan,now);
        if from!=plan.home {
            assert!(travel>0);let resident=self.residents.get_mut(&id).unwrap();self.due.remove(&(resident.next,id));
            resident.place=Place::Travel{from,to:plan.home,depart:now,arrive:now.saturating_add(travel)};
            resident.next=now.saturating_add(travel);resident.activity=None;resident.release=None;self.due.insert((resident.next,id));
        }
    }
    pub fn seed(&mut self,id:ResidentId,plan:Plan,now:Tick) {
        plan.validate();
        if let Some(resident)=self.residents.get_mut(&id) {
            if resident.plan!=plan && resident.place==(Place::At{lot:resident.plan.home}) && resident.next>now {
                self.due.remove(&(resident.next,id));let (next,activity,release)=plan.next_visit(now);
                resident.next=next;resident.activity=activity;resident.release=release;self.due.insert((next,id));
            }
            resident.plan=plan;return;
        }
        let (next,activity,release)=plan.next_visit(now);
        self.residents.insert(id,Resident{plan,place:Place::At{lot:plan.home},next,transitions:0,activity,release});self.due.insert((next,id));
    }
    /// Current position on a straight coarse segment; road/path refinement belongs
    /// to the detailed handoff. Original endpoints/dates survive save/load.
    pub fn segment(&self,id:ResidentId,now:Tick)->Option<(LotId,LotId,f64)> {
        match self.residents.get(&id)?.place {
            Place::At{lot}=>Some((lot,lot,1.0)),
            Place::Travel{from,to,depart,arrive}=>Some((from,to,((now.saturating_sub(depart)) as f64/(arrive-depart) as f64).clamp(0.0,1.0))),
        }
    }
    pub fn advance(&mut self,now:Tick,budget:usize)->Work {self.advance_with(now,budget,|_,_,_,_|{})}
    /// Account for the old interval before changing place. Consumers keep their
    /// own durable cursor; overdue events therefore cannot rewrite past needs.
    pub fn advance_with(&mut self,now:Tick,budget:usize,mut interval:impl FnMut(ResidentId,&Resident,Tick,bool))->Work {
        if self.budget_tick!=Some(now) {self.budget_tick=Some(now);self.spent=0;}
        let mut processed=0;
        while self.spent<budget {
            let Some(&(at,id))=self.due.first() else {break};if at>now {break;}
            self.due.remove(&(at,id));
            let Some(resident)=self.residents.get_mut(&id) else {continue};
            interval(id,resident,at,true);
            let plan=resident.plan;
            match resident.place {
                Place::At{lot}=> {
                    let to=if lot==plan.home {resident.activity.map_or(plan.destination,|d|d.destination)} else {plan.home};
                    let travel=resident.activity.map_or(plan.travel,|d|d.travel);
                    if lot!=plan.home && let (Some(activity),Some(ended))=(resident.activity,resident.release) {
                        let duration=if activity.end>activity.start {activity.end-activity.start} else {DAY-activity.start+activity.end};
                        self.completed.push_back(Completed{resident:id,purpose:activity.purpose,started:ended.saturating_sub(duration),ended});
                    }
                    resident.place=Place::Travel{from:lot,to,depart:at,arrive:at.saturating_add(travel)};
                    resident.next=at.saturating_add(travel);
                }
                Place::Travel{to,..}=> {
                    resident.place=Place::At{lot:to};
                    if to==plan.home {
                        let (next,activity,release)=plan.next_visit(at);resident.next=next;resident.activity=activity;resident.release=release;
                    } else {resident.next=resident.release.unwrap_or_else(||at.saturating_add(plan.stay)).max(at);}

                }
            }
            resident.transitions+=1;self.due.insert((resident.next,id));self.spent+=1;processed+=1;
        }
        let overdue=self.due.range(..=(now,u64::MAX)).count();
        Work{processed,overdue,oldest_delay:self.due.first().filter(|(at,_)|*at<=now).map_or(0,|(at,_)|now-*at)}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn plan(id:u64)->Plan {Plan{home:10+id%50,destination:100+id%25,departure:10200+id%1200,travel:150+id%100,stay:900+id%300,work:None,school:None}}
    #[test]
    fn thousand_residents_execute_distributed_journeys_and_restore_in_flight() {
        let mut city=City::default();for id in 0..1000 {city.seed(id,plan(id),0);}
        // Advance the real event machine, not just insertion/serialization.
        for now in 1..=10700 {let work=city.advance(now,32);assert!(work.processed<=32);}
        assert!(city.residents.values().any(|r|matches!(r.place,Place::Travel{..})));
        assert!(city.residents.values().any(|r|matches!(r.place,Place::At{lot} if lot>=100)));
        let json=serde_json::to_string(&city).unwrap();let mut restored:City=serde_json::from_str(&json).unwrap();
        for now in 10701..=DAY*2 {
            assert_eq!(city.advance(now,32),restored.advance(now,32));
        }
        assert_eq!(city.residents,restored.residents);
        assert!(city.residents.values().all(|r|r.transitions==8));
        assert!(city.residents.values().all(|r|r.place==Place::At{lot:r.plan.home}));
    }
    #[test]
    fn delayed_events_keep_dates_and_budget_across_extra_frames_and_saves() {
        let mut city=City::default();for id in 0..40 {city.seed(id,plan(id),0);}
        let first=city.advance(12000,3);assert_eq!(first.processed,3);assert!(first.overdue>0);
        assert_eq!(city.advance(12000,3).processed,0);
        let mut city:City=serde_json::from_str(&serde_json::to_string(&city).unwrap()).unwrap();assert_eq!(city.advance(12000,3).processed,0);
        for now in 12001..12200 {city.advance(now,3);}
        assert!(city.residents.values().all(|r|r.transitions==4));
        assert!(city.residents.values().all(|r|r.next>=DAY+10200));
    }
    #[test]
    fn detailed_ownership_removes_pending_events_without_replaying_them() {
        let mut city=City::default();city.seed(7,plan(7),0);city.advance(10207,4);assert_eq!(city.residents[&7].transitions,1);
        let journey=city.take(7).unwrap();assert_eq!(journey.place,Place::Travel{from:17,to:107,depart:10207,arrive:10364});
        assert_eq!(city.advance(20000,100).processed,0);assert!(city.residents.is_empty());
    }
}

#[cfg(test)]
mod calendar_tests {
    use super::*;
    fn plan()->Plan {Plan{home:1,destination:2,departure:10200,travel:200,stay:1200,work:None,school:None}}
    fn duty(purpose:Purpose,start:Tick,end:Tick)->Commitment {Commitment{purpose,destination:3,days:31,start,end,travel:250,cancelled_day:None}}
    #[test]
    fn school_weekend_and_teen_after_school_job_do_not_overlap_leisure() {
        let mut city=City::default();let mut p=plan();p.school=Some(duty(Purpose::School,5400,8400));p.work=Some(duty(Purpose::Work,9600,11400));city.seed(1,p,0);
        assert_eq!(city.residents[&1].next,5150);
        for now in 1..DAY*7 {city.advance(now,16);}
        let r=&city.residents[&1];assert_eq!(r.transitions,48); // 5*(school+job)*4 + weekend leisure*4
        assert_eq!(r.place,Place::At{lot:1});assert_eq!(r.next,DAY*7+5150);
    }
    #[test]
    fn night_shift_keeps_release_date_across_checkpoint_and_cancels_clashing_outing() {
        let mut p=plan();p.work=Some(duty(Purpose::Work,13200,3600));p.departure=12600;
        let mut city=City::default();city.seed(1,p,0);assert_eq!(city.residents[&1].next,12950);
        city.advance(13400,16);assert_eq!(city.residents[&1].release,Some(DAY+3600));
        let mut city:City=serde_json::from_str(&serde_json::to_string(&city).unwrap()).unwrap();
        city.advance(DAY+3600,16);assert!(matches!(city.residents[&1].place,Place::Travel{to:1,..}));
        city.advance(DAY+3850,16);assert_eq!(city.residents[&1].place,Place::At{lot:1});
    }
    #[test]
    fn calendar_cancellation_replans_only_future_departure_without_erasing_overdue_history() {
        let mut p=plan();let school=duty(Purpose::School,5400,9000);p.school=Some(school);let mut city=City::default();city.seed(1,p,0);
        p.school.as_mut().unwrap().cancelled_day=Some(0);city.seed(1,p,1000);assert_eq!(city.residents[&1].next,10200);
        let mut city=City::default();p.school=Some(school);city.seed(1,p,0);p.school=None;city.seed(1,p,6000);
        assert_eq!(city.residents[&1].next,5150);city.advance(6000,8);assert_eq!(city.residents[&1].release,Some(9000));
    }
}

#[cfg(test)]
mod return_tests {
    use super::*;
    #[test]
    fn rejoining_retired_visitor_returns_home_before_resuming_calendar() {
        let plan=Plan{home:1,destination:2,departure:10200,travel:150,stay:1200,work:None,school:None};let mut city=City::default();
        city.rejoin(7,plan,12000,2,175);assert_eq!(city.residents[&7].next,12175);
        let mut city:City=serde_json::from_str(&serde_json::to_string(&city).unwrap()).unwrap();assert_eq!(city.advance(12174,10).processed,0);
        city.advance(12175,10);assert_eq!(city.residents[&7].place,Place::At{lot:1});assert_eq!(city.residents[&7].next,DAY+10200);
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    #[test]
    fn completion_survives_dispatch_save_and_is_consumed_exactly_once() {
        let activity=Commitment{purpose:Purpose::School,destination:3,days:31,start:5400,end:9000,travel:250,cancelled_day:None};
        let plan=Plan{home:1,destination:2,departure:10200,travel:200,stay:1200,work:None,school:Some(activity)};
        let mut city=City::default();city.seed(7,plan,0);city.advance(9000,32);assert_eq!(city.completed_count(),1);
        let mut city:City=serde_json::from_str(&serde_json::to_string(&city).unwrap()).unwrap();
        assert_eq!(city.pop_completed(),Some(Completed{resident:7,purpose:Purpose::School,started:5400,ended:9000}));
        assert_eq!(city.pop_completed(),None);city.advance(9000,32);assert_eq!(city.pop_completed(),None);
        let mut city:City=serde_json::from_str(&serde_json::to_string(&city).unwrap()).unwrap();assert_eq!(city.pop_completed(),None);
    }
}

#[cfg(test)]
mod interval_tests {
    use super::*;
    #[test]
    fn delayed_intervals_are_reported_before_each_original_transition() {
        let p=Plan{home:1,destination:2,departure:100,travel:20,stay:30,work:None,school:None};let mut city=City::default();city.seed(1,p,0);
        let mut spans=Vec::new();city.advance_with(200,8,|id,r,at,_|spans.push((id,r.place,at)));
        assert_eq!(spans,vec![(1,Place::At{lot:1},100),(1,Place::Travel{from:1,to:2,depart:100,arrive:120},120),(1,Place::At{lot:2},150),(1,Place::Travel{from:2,to:1,depart:150,arrive:170},170)]);
        assert_eq!(city.advance_with(200,8,|_,_,_,_|panic!("no replay")).processed,0);
    }
}
