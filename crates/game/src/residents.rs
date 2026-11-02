//! Stable city checkpoints are independent of which residents currently have scene models.
//! ECS owns detailed actors; dormant played households keep their own snapshots. The city
//! registry owns other residents and retains the last save checkpoint for detailed actors.
use bevy::prelude::*;
use crate::{clock::GameClock, story::TownStory};

pub(crate) fn seed_residents(clock: Res<GameClock>, town: Option<Res<crate::premade::TownPremades>>, mut story: ResMut<TownStory>) {
    story.migrate_models();
    let Some(town) = town else { return };
    if !town.is_changed() && !clock.restored && !story.residents.records.is_empty() { return; }
    let mut links: std::collections::HashMap<u64, Vec<crate::save::SavedRel>> = std::collections::HashMap::new();
    for relation in &town.0.relationships {
        let (status, romance) = match relation.state.as_str() {
            "Spouse" => ("Married", 80.0), "Fiancee" => ("Engaged", 70.0),
            "Partner" => ("Partner", 60.0), "Romantic Interest" => ("None", 40.0),
            "Ex" | "Ex Spouse" => ("Ex", 0.0), _ => ("None", 0.0),
        };
        for (owner, other) in [(relation.a, relation.b), (relation.b, relation.a)] {
            let mut row = new_relationship(other, status, clock.minutes);
            row.friendship = relation.liking; row.romance = romance; row.romantic = romance > 0.0; row.kissed = romance > 0.0;
            links.entry(owner).or_default().push(row);
        }
    }
    for household in town.0.households.iter().filter(|h| !h.name.to_ascii_lowercase().contains("ghost")) {
        let home = story.home_of(household);
        for person in household.members.iter().filter(|p| !p.first_name.is_empty()) {
            let missing = story.residents.records.get(&person.id).is_none_or(|r| r.state.is_none());
            if missing {
                let sim = crate::premade::to_sim(person);
                let mut state = crate::save::saved_look(&sim);
                state.motives = crate::sim::Motives::default().0;
                state.skills = person.skills.iter().map(|(skill, level)| (skill.clone(), (*level).clamp(0, 10) as f32)).collect();
                state.job = person.career.as_ref().and_then(|(career, level)| saved_job(career, (*level).max(1) as u32));
                state.relationships = links.get(&person.id).cloned().unwrap_or_default();
                for kin in person.parents.iter().chain(&person.children) {
                    if let Some(row) = state.relationships.iter_mut().find(|r| r.with == *kin) { row.friendship = row.friendship.max(70.0); }
                    else { let mut row = new_relationship(*kin, "None", clock.minutes); row.friendship = 70.0; row.romance = 0.0; row.kissed = false; row.romantic = false; state.relationships.push(row); }
                }
                // Initial family links use stable IDs even where no partner has a body.
                if let Some((id, status)) = person.spouse.map(|id| (id, "Married")).or_else(|| person.partner.map(|id| (id, "Partner"))) {
                    if let Some(row) = state.relationships.iter_mut().find(|r| r.with == id) { row.status = status.into(); row.friendship = row.friendship.max(80.0); row.romance = row.romance.max(80.0); row.kissed = true; row.romantic = true; }
                    else { state.relationships.push(new_relationship(id, status, clock.minutes)); }
                }
                story.residents.seed(sim.id, Some(household.id), home, state);
            }
            let record = story.residents.records.get_mut(&person.id).unwrap();
            if record.household.is_none() { record.household = Some(household.id); record.home = home; }
            let state = record.state.as_ref().unwrap();
            let model = record.model.get_or_insert_with(|| {
                let age = crate::save::age_from_name(&state.age);
                let span = crate::aging::stage_days(age);
                let span = if span.is_finite() { span } else { 17.0 };
                s3sim::residents::ResidentModel {
                    age: Some(state.age.clone()),
                    days: state.aging.map_or_else(|| ((state.look % 1_000_003) as f64 / 1_000_003.0 * span as f64 * 0.6).floor() as f32, |a| a.0),
                    partner: state.relationships.iter().find(|r| matches!(r.status.as_str(), "Partner" | "Engaged")).map(|r| r.with),
                    spouse: state.relationships.iter().find(|r| r.status == "Married").map(|r| r.with),
                    job: state.job.as_ref().map(|j| (j.track.clone(), j.level as u32 + 1)),
                    ..Default::default()
                }
            });
            if model.age.is_none() { model.age = Some(state.age.clone()); }
            if model.elder_risk.is_none() && !model.legacy_fixed_lifespan {
                if record.checkpoint_at.is_some() && let Some((_, span)) = state.aging {
                    model.elder_span = span; model.elder_risk = state.elder_risk;
                    model.legacy_fixed_lifespan = state.elder_risk.is_none();
                } else { model.elder_risk = Some(s3sim::residents::initial_elder_risk(person.id)); }
            }
            let model = model.clone();
            if let Some(state) = &mut record.state { apply_model(state, &model, clock.minutes); }
        }
    }
}

fn saved_job(career: &str, level: u32) -> Option<crate::save::SavedJob> {
    let track = crate::story::track(career)?;
    let index = (level.saturating_sub(1) as usize).min(track.levels().len().saturating_sub(1));
    Some(crate::save::SavedJob { track: track.name.into(), level: index, performance: 0.0,
        last_day: None, branch: String::new(), tone: None, boss: None, coworkers: Vec::new(),
        highest: index, raise: 0, warned_at: None, tally: Default::default() })
}

fn new_relationship(with: u64, status: &str, now: f64) -> crate::save::SavedRel {
    crate::save::SavedRel { with, friendship: 80.0, romance: 80.0, status: status.into(), kissed: true,
        last: now, stc: None, enemy: false, romantic: true, betrayed: None, known_traits: Vec::new() }
}

/// Read the canonical city model into a personal snapshot, preserving richer details.
pub(crate) fn apply_model(state: &mut crate::save::SavedSim, model: &s3sim::residents::ResidentModel, now: f64) {
    if let Some(age) = &model.age {
        if state.age != *age {
            state.outfit.clear(); state.accessories = None; state.eyeliner = None; state.blush = None;
            state.brows = None; state.skin_details.clear(); state.body_hair.clear(); state.designs.clear(); state.styles.clear();
            state.wearing = None; state.outfit_chosen = false;
            let target = crate::save::age_from_name(age);
            let mut traits: Vec<_> = state.traits.iter().filter_map(|t| crate::life::Trait::from_name(t)).collect();
            use rand::seq::IndexedRandom;
            let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(state.look ^ target as u64);
            while traits.len() < crate::life::trait_slots(target) {
                let choices: Vec<_> = crate::life::Trait::ALL.into_iter().filter(|t| t.allowed_at(target) && !traits.contains(t) && t.compatible(&traits)).collect();
                let Some(&t) = choices.choose(&mut rng) else { break }; traits.push(t);
            }
            state.traits = traits.into_iter().map(|t| t.name().to_string()).collect();
        }
        state.age = age.clone();
    }
    state.aging = Some((model.days, model.elder_span)); state.elder_risk = model.elder_risk;
    match &model.job {
        None => state.job = None,
        Some((career, level)) => {
            if let Some(next) = saved_job(career, *level) {
                if let Some(current) = &mut state.job && current.track == next.track {
                    if current.level != next.level { current.level = next.level; current.performance = 0.0; }
                    current.highest = current.highest.max(current.level);
                } else { state.job = Some(next); }
            }
        }
    }
    for rel in &mut state.relationships {
        if matches!(rel.status.as_str(), "Partner" | "Engaged" | "Married")
            && model.partner != Some(rel.with) && model.spouse != Some(rel.with) { rel.status = "Ex".into(); rel.romantic = false; }
    }
    if let Some((id, status)) = model.spouse.map(|id| (id, "Married")).or_else(|| model.partner.map(|id| (id, "Partner"))) {
        if let Some(rel) = state.relationships.iter_mut().find(|r| r.with == id) {
            if status == "Married" || !matches!(rel.status.as_str(), "Partner" | "Engaged") { rel.status = status.into(); }
            rel.romantic = true;
        } else { state.relationships.push(new_relationship(id, status, now)); }
    }
}

pub(crate) fn projected_state(story: &TownStory, id: &u64, now: f64) -> Option<crate::save::SavedSim> {
    let record = story.residents.records.get(id)?;
    let mut state = record.state.as_ref()?.clone();
    if let Some(model) = story.model(id) { apply_model(&mut state, model, now); }
    Some(state)
}

pub(crate) fn sync_background(story: &mut TownStory, active: &std::collections::HashSet<u64>, now: f64) {
    let TownStory { residents, homes, .. } = story;
    for (id, resident) in &mut residents.records {
        if active.contains(id) { continue; }
        if let Some(household) = resident.household && let Some(home) = homes.get(&household) { resident.home = *home; }
        if let (Some(model), Some(state)) = (&resident.model, &mut resident.state) { apply_model(state, model, now); }
    }
}

/// Capture detailed gameplay after daily reconciliation, including input made while
/// paused before the city event subscriber has had another simulation tick.
pub(crate) fn checkpoint(story: &mut TownStory, state: &mut crate::save::SavedSim, now: f64) {
    let model = story.residents.model_or_insert_with(state.id, || s3sim::residents::ResidentModel {
        age: Some(state.age.clone()), elder_risk: Some(s3sim::residents::initial_elder_risk(state.id)), ..Default::default() });
    model.age = Some(state.age.clone());
    if let Some((days, span)) = state.aging { model.days = days; model.elder_span = span; model.elder_risk = state.elder_risk; model.legacy_fixed_lifespan = state.elder_risk.is_none(); }
    model.job = state.job.as_ref().map(|j| (j.track.clone(), j.level as u32 + 1));
    model.spouse = state.relationships.iter().find(|r| r.status == "Married").map(|r| r.with);
    model.partner = state.relationships.iter().find(|r| matches!(r.status.as_str(), "Partner" | "Engaged")).map(|r| r.with);
    story.residents.checkpoint(state.id, state.clone(), s3sim::tick_at(now));
}

pub(crate) fn restore_background(entity: &mut EntityCommands, background: &s3sim::residents::BackgroundState) {
    entity.remove::<(crate::interact::Visitor, crate::visit::OnLot, crate::visit::LotGuest, crate::homes::Resident)>();
    entity.insert(crate::town::Townie { next_walk: background.next_walk, walking: background.walk.is_some() });
    if let Some(walk) = &background.walk {
        let style = if walk.jogging { crate::nav::WalkStyle::Jog } else { crate::nav::WalkStyle::Walk };
        let mut path = crate::nav::PathFollow::new(vec![crate::nav::Waypoint { p: Vec2::from_array(walk.destination), level: 1, climb: None, spiral: None }]).with_style(style);
        path.speed = walk.speed;
        entity.remove::<crate::interact::OffLot>().insert((path, Visibility::Inherited, crate::nav::Floor(1)));
    } else {
        entity.remove::<crate::nav::PathFollow>().insert((crate::interact::OffLot, Visibility::Hidden, crate::nav::Floor(1)));
    }
}

/// Full checkpoints queued when taking up a town household (there is no PendingLoad then).
#[derive(Resource)]
pub(crate) struct RestoreResidents(pub Vec<crate::save::SavedSim>);

pub(crate) fn restore_arrivals(mut commands: Commands, pending: Option<Res<RestoreResidents>>,
    clock: Res<GameClock>, world: Res<crate::loading::CurrentWorld>,
    building: Option<Res<crate::building::ActiveBuilding>>, mut people: crate::save::RestorePeople,
    roles: Query<(Has<crate::sim::HouseholdMember>, Has<crate::sim::Selected>)>) {
    let Some(pending) = pending else { return };
    let mut snapshots = pending.0.clone();
    let ids = people.iter().map(|p| (p.0, p.1.id)).collect();
    for snapshot in &mut snapshots {
        let Some((e, _, tf, floor, ..)) = people.iter().find(|p| p.1.id == snapshot.id) else { continue };
        let Ok((member, selected)) = roles.get(e) else { continue };
        // carry_over has just restored ties from played households. Keep those
        // missing from a cold profile when restoring the new household's details.
        if let Ok((_, _, _, _, _, _, _, relations)) = people.get(e) {
            for row in crate::save::saved_rels(relations, &ids) {
                if !snapshot.relationships.iter().any(|r| r.with == row.with) { snapshot.relationships.push(row); }
            }
        }
        snapshot.member = member; snapshot.selected = selected;
        // Taking over another family is an explicit move to its home, not travel back
        // to the previous household's scene. Their personal state still comes across.
        if member {
            snapshot.position = tf.translation.to_array(); snapshot.floor = floor.0;
            snapshot.world_location = None; snapshot.lot_role = None; snapshot.background = None;
            snapshot.whereabouts = "home".into();
        } else if snapshot.background.is_none() {
            snapshot.member = false; snapshot.selected = false; snapshot.whereabouts = "away".into();
            snapshot.world_location = None; snapshot.lot_role = None;
        }
    }
    let unmet = crate::save::restore_people(&mut commands, clock.minutes, true, &world, building.as_deref(), &mut people, &snapshots);
    commands.insert_resource(crate::ghosts::PendingRels(unmet));
    commands.remove_resource::<RestoreResidents>();
}

/// Stable ID bindings cover ordinary arrivals as well as ghosts. The index is rebuilt
/// only when actors change or a loaded scene replaces the previous one.
#[derive(Resource, Default)]
pub(crate) struct ResidentActors {
    pub by_id: std::collections::HashMap<u64, Entity>,
    pub by_entity: std::collections::HashMap<Entity, u64>,
    arrivals: Vec<Entity>,
}

pub(crate) fn bind_contacts(
    clock: Res<GameClock>, story: Res<TownStory>,
    arrivals: Query<Entity, Added<crate::sim::Sim>>,
    mut removed: RemovedComponents<crate::sim::Sim>,
    everyone: Query<(Entity, &crate::sim::Sim)>,
    mut index: ResMut<ResidentActors>,
    mut relationships: Query<&mut crate::social::Relationships>,
    mut pending: ResMut<crate::ghosts::PendingRels>,
) {
    let removed_any = removed.read().count() != 0;
    let arrivals: Vec<_> = arrivals.iter().collect();
    if arrivals.is_empty() && !removed_any && !clock.restored { return; }
    index.arrivals = arrivals.clone();
    index.by_id = everyone.iter().map(|(e, s)| (s.id, e)).collect();
    index.by_entity = index.by_id.iter().map(|(id, e)| (*e, *id)).collect();
    let alive: std::collections::HashSet<_> = index.by_entity.keys().copied().collect();
    pending.0.retain(|(owner, _)| alive.contains(owner));
    let mut queued: std::collections::HashSet<_> = pending.0.iter().map(|(e, r)| (*e, r.with)).collect();
    for (owner, sim) in &everyone {
        if !clock.restored && !arrivals.contains(&owner) { continue; }
        let Some(state) = story.residents.records.get(&sim.id).and_then(|r| r.state.as_ref()) else { continue };
        let Ok(mut mine) = relationships.get_mut(owner) else { continue };
        for row in &state.relationships {
            if let Some(&target) = index.by_id.get(&row.with) {
                if !mine.0.contains_key(&target) { crate::save::restore_rel(mine.entry(target), row); }
            } else if queued.insert((owner, row.with)) { pending.0.push((owner, row.clone())); }
        }
    }
    pending.0.retain(|(owner, row)| {
        let Some(&target) = index.by_id.get(&row.with) else { return true };
        if let Ok(mut mine) = relationships.get_mut(*owner) {
            if !mine.0.contains_key(&target) { crate::save::restore_rel(mine.entry(target), row); }
        }
        false
    });
}

/// Independent event subscriber: city summaries see player changes without
/// consuming the events needed by wishes, dreams or moodlets, including paused input.
pub(crate) fn model_events(
    mut events: ResMut<crate::life::PendingEvents<crate::life::LifeEvent, 6>>,
    actors: Query<(Entity, &crate::sim::Sim, Option<&crate::aging::Aging>, Option<&crate::careers::Job>, &crate::social::Relationships, Has<crate::sim::HouseholdMember>)>,
    pending: Res<crate::ghosts::PendingRels>, index: Res<ResidentActors>, mut story: ResMut<TownStory>,
) {
    use crate::life::LifeEventKind as K;
    let changes: std::collections::HashMap<_, _> = events.take().into_iter().filter_map(|event| {
        let flags = match event.kind {
            K::Birthday => 1_u8,
            K::Promoted | K::Demoted | K::Fired | K::NewJob => 2,
            K::StartedDating | K::Engaged | K::Married | K::BrokeUp | K::MovedIn => 4,
            _ => return None,
        };
        Some((event.sim, flags))
    }).fold(std::collections::HashMap::new(), |mut changes, (entity, flags)| { *changes.entry(entity).or_default() |= flags; changes });
    for (e, sim, aging, job, rels, member) in &actors {
        let flags = changes.get(&e).copied().unwrap_or(0) | if member { 3 } else { 0 };
        if flags == 0 { continue; }
        let model = story.residents.model_or_insert_with(sim.id, || s3sim::residents::ResidentModel { age: Some(crate::save::age_name(sim.age).into()), ..Default::default() });
        if flags & 1 != 0 {
            let age_name = crate::save::age_name(sim.age);
            if model.age.as_deref() != Some(age_name) { model.age = Some(age_name.into()); }
            if let Some(age) = aging { model.days = age.days; model.elder_span = age.elder_span; model.elder_risk = age.elder_risk; model.legacy_fixed_lifespan = age.elder_risk.is_none(); }
        }
        if flags & 2 != 0 {
            let summary = job.map(|job| (crate::careers::careers()[job.track].name, job.level as u32 + 1));
            if model.job.as_ref().map(|(name, level)| (name.as_str(), *level)) != summary { model.job = summary.map(|(name, level)| (name.to_string(), level)); }
        }
        if flags & 4 != 0 {
            use crate::social::RelStatus as S;
            let linked = |status: fn(S) -> bool| rels.0.iter().find_map(|(target, rel)| status(rel.status).then(|| index.by_entity.get(target).copied()).flatten());
            model.spouse = linked(|s| s == S::Married).or_else(|| pending.0.iter().find(|(owner, r)| *owner == e && r.status == "Married").map(|(_, r)| r.with));
            model.partner = linked(|s| matches!(s, S::Partner | S::Engaged)).or_else(|| pending.0.iter().find(|(owner, r)| *owner == e && matches!(r.status.as_str(), "Partner" | "Engaged")).map(|(_, r)| r.with));
        }
    }
}

/// Apply a completed city day to the handful of detailed NPCs. No bodies are
/// created for cold residents, and no rich career/relationship data is reset.
pub(crate) fn reconcile_daily(
    mut commands: Commands, clock: Res<GameClock>, story: Res<TownStory>,
    mut seen: Local<Option<u32>>, index: Res<ResidentActors>,
    mut people: Query<(Entity, &mut crate::sim::Sim, Option<&mut crate::aging::Aging>, Option<&mut crate::careers::Job>, &mut crate::social::Relationships), Without<crate::sim::HouseholdMember>>,
    mut pending: ResMut<crate::ghosts::PendingRels>, dormant: Res<crate::household::Dormant>,
) {
    let new_day = *seen != Some(story.day) || clock.restored;
    if !new_day && !index.is_changed() { return; }
    *seen = Some(story.day);
    let protected: std::collections::HashSet<_> = dormant.member_ids().into_iter().collect();
    for (e, mut sim, aging, job, mut rels) in &mut people {
        if protected.contains(&sim.id) || (!new_day && !index.arrivals.contains(&e)) { continue; }
        let Some(model) = story.model(&sim.id).filter(|m| m.present()) else { continue };
        if let Some(age) = &model.age {
            let next = crate::save::age_from_name(age);
            if next != sim.age {
                let mut state = crate::save::saved_look(&sim); apply_model(&mut state, model, clock.minutes);
                *sim = crate::save::SaveGame::sim(&state);
                commands.entity(e).insert(crate::aging::NeedsNewBody);
            }
        }
        let next_age = crate::aging::Aging { days: model.days, elder_span: model.elder_span, elder_risk: model.elder_risk };
        if let Some(mut age) = aging { *age = next_age; } else { commands.entity(e).insert(next_age); }
        match &model.job {
            None => { if job.is_some() { commands.entity(e).remove::<crate::careers::Job>(); } }
            Some((name, level)) => {
                if let Some(track) = crate::story::track(name).and_then(|t| crate::careers::careers().iter().position(|c| c.name == t.name)) {
                    let level = (level.saturating_sub(1) as usize).min(crate::careers::careers()[track].levels().len().saturating_sub(1));
                    if let Some(mut job) = job.filter(|j| j.track == track) {
                        if job.level != level { job.level = level; job.performance = 0.0; job.highest = job.highest.max(level); }
                    } else { let mut job = crate::careers::Job::new(track); job.level = level; job.highest = level; commands.entity(e).insert(job); }
                }
            }
        }
        for (&target, row) in &mut rels.0 {
            let id = index.by_entity.get(&target).copied();
            if matches!(row.status, crate::social::RelStatus::Partner | crate::social::RelStatus::Engaged | crate::social::RelStatus::Married)
                && id != model.spouse && id != model.partner { row.status = crate::social::RelStatus::Ex; row.romantic = false; }
        }
        pending.0.retain(|(owner, row)| *owner != e || !matches!(row.status.as_str(), "Partner" | "Engaged" | "Married") || Some(row.with) == model.spouse || Some(row.with) == model.partner);
        if let Some((id, status)) = model.spouse.map(|id| (id, "Married")).or_else(|| model.partner.map(|id| (id, "Partner"))) {
            if let Some(&target) = index.by_id.get(&id) {
                let row = rels.entry(target);
                if status == "Married" || !matches!(row.status, crate::social::RelStatus::Partner | crate::social::RelStatus::Engaged) { row.status = crate::save::status_from(status); }
                row.romantic = true;
            } else if let Some((_, row)) = pending.0.iter_mut().find(|(owner, row)| *owner == e && row.with == id) {
                if status == "Married" || !matches!(row.status.as_str(), "Partner" | "Engaged") { row.status = status.into(); }
                row.romantic = true;
            } else { pending.0.push((e, new_relationship(id, status, clock.minutes))); }
        }
    }
}
