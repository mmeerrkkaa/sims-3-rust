//! Durable coarse journeys between world lots. No scene entities or rendering.
//! This is an event queue, not a coalescible reconsideration agenda: overdue
//! departures and arrivals are consumed in order, with their original dates.
use std::collections::{BTreeMap,BTreeSet};
use serde::{Serialize,Deserialize};
use crate::{ResidentId,Tick,residents::LotId};
pub const DAY:Tick=14_400;
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Plan {pub home:LotId,pub destination:LotId,pub departure:Tick,pub travel:Tick,pub stay:Tick}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub enum Place {At{lot:LotId},Travel{from:LotId,to:LotId,depart:Tick,arrive:Tick}}
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Resident {pub plan:Plan,pub place:Place,pub next:Tick,pub transitions:u64}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct City {
    pub residents:BTreeMap<ResidentId,Resident>,
    due:BTreeSet<(Tick,ResidentId)>,
    budget_tick:Option<Tick>,spent:usize,
}
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub struct Work {pub processed:usize,pub overdue:usize,pub oldest_delay:Tick}
impl Plan {
    fn validate(self) {assert!(self.departure<DAY);assert!(self.travel>0 && self.stay>0);assert_ne!(self.home,self.destination);}
    fn next_departure(self,after:Tick)->Tick {
        let today=after/DAY*DAY+self.departure;
        if today>after {today} else {today.saturating_add(DAY)}
    }
}
impl City {
    /// Reconcile only cold residents. A detailed actor's state must be captured
    /// before it enters this roster; removing it transfers ownership out of here.
    pub fn retain(&mut self,mut keep:impl FnMut(ResidentId)->bool) {
        self.residents.retain(|id,_|keep(*id));self.due.retain(|(_,id)|self.residents.contains_key(id));
    }
    /// Transfer the full pending journey to detailed ownership atomically.
    pub fn take(&mut self,id:ResidentId)->Option<Resident> {
        let resident=self.residents.remove(&id)?;self.due.remove(&(resident.next,id));Some(resident)
    }
    pub fn seed(&mut self,id:ResidentId,plan:Plan,now:Tick) {
        plan.validate();
        if let Some(resident)=self.residents.get_mut(&id) {resident.plan=plan;return;}
        let next=plan.next_departure(now);self.residents.insert(id,Resident{plan,place:Place::At{lot:plan.home},next,transitions:0});self.due.insert((next,id));
    }
    /// Current position on a straight coarse segment; road/path refinement belongs
    /// to the detailed handoff. Original endpoints/dates survive save/load.
    pub fn segment(&self,id:ResidentId,now:Tick)->Option<(LotId,LotId,f64)> {
        match self.residents.get(&id)?.place {
            Place::At{lot}=>Some((lot,lot,1.0)),
            Place::Travel{from,to,depart,arrive}=>Some((from,to,((now.saturating_sub(depart)) as f64/(arrive-depart) as f64).clamp(0.0,1.0))),
        }
    }
    pub fn advance(&mut self,now:Tick,budget:usize)->Work {
        if self.budget_tick!=Some(now) {self.budget_tick=Some(now);self.spent=0;}
        let mut processed=0;
        while self.spent<budget {
            let Some(&(at,id))=self.due.first() else {break};if at>now {break;}
            self.due.remove(&(at,id));
            let Some(resident)=self.residents.get_mut(&id) else {continue};
            let plan=resident.plan;
            match resident.place {
                Place::At{lot}=> {
                    let to=if lot==plan.home {plan.destination} else {plan.home};
                    resident.place=Place::Travel{from:lot,to,depart:at,arrive:at.saturating_add(plan.travel)};
                    resident.next=at.saturating_add(plan.travel);
                }
                Place::Travel{to,..}=> {
                    resident.place=Place::At{lot:to};
                    resident.next=if to==plan.home {plan.next_departure(at)} else {at.saturating_add(plan.stay)};
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
    fn plan(id:u64)->Plan {Plan{home:10+id%50,destination:100+id%25,departure:10200+id%1200,travel:150+id%100,stay:900+id%300}}
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
