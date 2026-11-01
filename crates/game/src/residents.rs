//! Stable city checkpoints are independent of which residents currently have scene models.
//! ECS owns detailed actors; dormant played households keep their own snapshots. The city
//! registry owns other residents and retains the last save checkpoint for detailed actors.
use bevy::prelude::*;
use crate::{clock::GameClock, story::TownStory};

pub(crate) fn seed_residents(clock: Res<GameClock>, town: Option<Res<crate::premade::TownPremades>>, mut story: ResMut<TownStory>) {
    let Some(town) = town else { return };
    if !town.is_changed() && !clock.restored && !story.residents.records.is_empty() { return; }
    for household in town.0.households.iter().filter(|h| !h.name.to_ascii_lowercase().contains("ghost")) {
        let home = story.home_of(household);
        for person in household.members.iter().filter(|p| !p.first_name.is_empty()) {
            if let Some(record) = story.residents.records.get_mut(&person.id) {
                if record.household.is_none() { record.household = Some(household.id); record.home = home; }
                continue;
            }
            let sim = crate::premade::to_sim(person);
            let mut state = crate::save::saved_look(&sim);
            state.motives = crate::sim::Motives::default().0;
            state.skills = person.skills.iter().map(|(skill, level)| (skill.clone(), (*level).clamp(0, 10) as f32)).collect();
            if let Some((career, level)) = &person.career {
                state.job = saved_job(career, *level as u32);
            }
            story.residents.seed(sim.id, Some(household.id), home, state);
        }
    }
}

fn saved_job(career: &str, level: u32) -> Option<crate::save::SavedJob> {
    let track = crate::story::track(career)?;
    Some(crate::save::SavedJob {
        track: track.name.into(), level: level.saturating_sub(1) as usize, performance: 0.0,
        last_day: None, branch: String::new(), tone: None, boss: None, coworkers: Vec::new(),
        highest: level.saturating_sub(1) as usize, raise: 0, warned_at: None, tally: Default::default(),
    })
}

/// Apply the daily background model only where ECS or a dormant played household is not
/// authoritative. Existing detailed needs/skills/inventory are retained in the checkpoint.
pub(crate) fn sync_background(story: &mut TownStory, active: &std::collections::HashSet<u64>) {
    let TownStory { sims, residents, homes, .. } = story;
    for (id, resident) in &mut residents.records {
        if active.contains(id) { continue; }
        if let Some(household) = resident.household && let Some(home) = homes.get(&household) {
            resident.home = *home;
        }
        let Some(town) = sims.get(id) else { continue };
        if let Some(age) = &town.age { resident.state.age = age.clone(); }
        if let Some((days, _)) = resident.state.aging.as_mut() { *days = town.days; }
        if let Some((career, level)) = &town.job {
            let changed = resident.state.job.as_ref().is_none_or(|j| j.track != *career || j.level != level.saturating_sub(1) as usize);
            if changed { resident.state.job = saved_job(career, *level); }
        }
    }
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
    for snapshot in &mut snapshots {
        let Some((e, _, tf, floor, ..)) = people.iter().find(|p| p.1.id == snapshot.id) else { continue };
        let Ok((member, selected)) = roles.get(e) else { continue };
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
