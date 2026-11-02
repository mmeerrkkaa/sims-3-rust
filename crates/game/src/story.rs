//! Story progression: the rest of the town lives on while the household plays. Once a day the
//! world's other Sims grow older (the oldest passing away in time), single grown-ups pair off
//! and couples marry, married couples have babies, grown-ups find jobs and are promoted, and
//! families move house, in and out of houses and away (`storymoves`).
//! The household hears about it when it's someone they know, and the morning paper carries
//! the town's news. What has changed is kept in saves and applied to the town's Sims as the
//! world loads.

use std::collections::BTreeMap;

use bevy::prelude::*;
use rand::Rng;
use rand::seq::{IndexedRandom, SliceRandom};
use serde::{Deserialize, Serialize};

use crate::PlayMode;
use crate::clock::GameClock;
use crate::interact::Notifications;
use crate::sim::{Age, HouseholdMember, Sim};
use crate::social::Relationships;

pub struct StoryPlugin;

impl Plugin for StoryPlugin {
    fn build(&self, app: &mut App) {
        crate::life::buffer_events::<crate::life::LifeEvent, 6>(app);
        app.init_resource::<TownStory>()
            .init_resource::<crate::residents::ResidentActors>()
            .init_resource::<crate::residents::MaterializeRequests>()
            .init_resource::<crate::city_mobility::MobilityWorkload>()
            .add_systems(OnEnter(crate::AppState::Loading),|mut work:ResMut<crate::city_mobility::MobilityWorkload>|*work=default())
            .add_systems(Update,crate::city_mobility::reset_frame.in_set(crate::clock::SimPhase::Advance))
            .add_systems(Update,crate::city_mobility::handoff.after(crate::residents::reconcile_daily).in_set(crate::clock::SimPhase::Prepare).run_if(in_state(PlayMode::Live)))
            .add_systems(crate::clock::SimTick,crate::city_mobility::advance.after(crate::residents::reconcile_daily).in_set(crate::aging::LifecyclePhase::Town))
            .init_resource::<crate::resident_bodies::BodyCache>()
            .add_systems(OnEnter(crate::AppState::Loading),crate::social_state::clear)
            .add_systems(Update,crate::social_state::restore.after(crate::residents::reconcile_daily).in_set(crate::clock::SimPhase::Prepare).run_if(in_state(PlayMode::Live)))
            .add_systems(OnEnter(crate::AppState::Loading), |mut cache:ResMut<crate::resident_bodies::BodyCache>| *cache=default())
            .add_systems(PostUpdate, crate::resident_bodies::maintain.before(bevy::ui::UiSystems::Prepare).run_if(in_state(PlayMode::Live)))
            .add_systems(OnEnter(crate::AppState::Loading), |mut requests:ResMut<crate::residents::MaterializeRequests>| *requests=default())
            .add_systems(Update, (prepare_story, merge_created, crate::residents::seed_residents, crate::residents::register_graves, crate::residents::resolve_calls, crate::residents::materialize_requested, crate::residents::prepare_personal_state, crate::residents::hydrate_new_actors, crate::residents::bind_contacts, crate::residents::resolve_calls, crate::residents::reconcile_daily).chain().run_if(in_state(PlayMode::Live)).in_set(crate::clock::SimPhase::Prepare))
            // Actors spawned after Prepare must finish their personal handoff
            // before a same-frame save can checkpoint their temporary defaults.
            .add_systems(Update, (crate::residents::prepare_personal_state, crate::residents::hydrate_new_actors, crate::residents::bind_contacts).chain()
                .in_set(crate::clock::SimPhase::Save).before(crate::save::save_game).run_if(in_state(PlayMode::Live)))
            .add_systems(crate::clock::SimTick, (merge_created, crate::residents::seed_residents, crate::life::capture_subscriber::<crate::life::LifeEvent, 6>, crate::residents::model_events, progress, crate::residents::reconcile_daily, read_the_news).chain().in_set(crate::aging::LifecyclePhase::Town))
            .add_systems(crate::clock::SimTick, (crate::life::capture_subscriber::<crate::life::LifeEvent, 6>, crate::residents::model_events).chain().in_set(crate::wishes::AspirationPhase::Initialize));
        #[cfg(local_validation)]
        app.init_resource::<crate::npc_ownership_test::CityMobilityProbe>()
            .add_systems(Update,crate::npc_ownership_test::city_mobility_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(||std::env::var_os("CITY_MOBILITY_TEST").is_some() || std::env::var_os("CITY_MOBILITY_RELOAD").is_some())))
            .init_resource::<crate::npc_ownership_test::SocialSaveProbe>()
            .add_systems(Update,crate::npc_ownership_test::social_save_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(||std::env::var_os("SOCIAL_SAVE_TEST").is_some() || std::env::var_os("SOCIAL_SAVE_RELOAD").is_some())))
            .init_resource::<crate::npc_ownership_test::DurableMeetingProbe>()
            .add_systems(Update,crate::npc_ownership_test::durable_meeting_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(||std::env::var_os("DURABLE_MEETING_TEST").is_some())))
            .init_resource::<crate::npc_ownership_test::BodyCacheProbe>()
            .add_systems(Update, crate::npc_ownership_test::body_cache_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(||std::env::var_os("BODY_CACHE_TEST").is_some() || std::env::var_os("BODY_CACHE_RELOAD").is_some())))
            .init_resource::<crate::npc_ownership_test::ResidentRequestProbe>()
            .add_systems(Update, crate::npc_ownership_test::resident_request_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(||std::env::var_os("RESIDENT_REQUEST_TEST").is_some() || std::env::var_os("RESIDENT_REQUEST_RELOAD").is_some())))
            .init_resource::<crate::npc_ownership_test::ResidentHandoffProbe>()
            .add_systems(Update, crate::npc_ownership_test::resident_handoff_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(|| std::env::var_os("RESIDENT_HANDOFF_TEST").is_some() || std::env::var_os("RESIDENT_HANDOFF_RELOAD").is_some())))
            .init_resource::<crate::npc_ownership_test::CanonicalResidentProbe>()
            .add_systems(Update, crate::npc_ownership_test::canonical_resident_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(|| std::env::var_os("CANONICAL_RESIDENT_TEST").is_some() || std::env::var_os("CANONICAL_RESIDENT_RELOAD").is_some())))
            .init_resource::<crate::npc_ownership_test::ResidentStoreProbe>()
            .add_systems(Update, crate::npc_ownership_test::resident_store_frame.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(|| std::env::var_os("RESIDENT_STORE_TEST").is_some())))
            .add_systems(Update, crate::npc_ownership_test::resident_store_reload.in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live).and_then(|| std::env::var_os("RESIDENT_STORE_RELOAD").is_some())));
    }
}

fn prepare_story(clock: Res<GameClock>, mut story: ResMut<TownStory>, members: Query<(), With<HouseholdMember>>) {
    if members.is_empty() { return; }
    let today = clock.day();
    if !story.day_initialized {
        // Older saves with a positive day already contain a valid checkpoint.
        if story.day == 0 { story.day = today; }
        story.day_initialized = true;
    }
    if clock.restored && story.day > today { story.day = today; }
}

/// How the town's Sims have moved on since the world began.
#[derive(Resource, Clone, Default, Debug, Serialize, Deserialize)]
pub struct TownStory {
    /// Persistent decisions keyed by resident ID, independent of ECS entity lifetimes.
    #[serde(default)]
    pub conversations: s3sim::ConversationPlans,
    #[serde(default)]
    pub mobility:s3sim::mobility::City,
    /// City checkpoints do not imply that every resident owns a rendered actor.
    #[serde(default)]
    pub residents: s3sim::residents::Residents<crate::save::SavedSim>,
    /// The last day the town completed; pending days remain until processed.
    pub day: u32,
    /// Distinguishes a real day zero from the legacy uninitialized sentinel.
    #[serde(default)]
    pub day_initialized: bool,
    /// Holiday notices and bonuses already delivered, shared across household switches.
    #[serde(default)]
    pub holidays: crate::holidays::HolidayHistory,
    /// What has changed for each of the world's Sims, by id.
    #[serde(default, rename = "sims", skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) legacy_sims: BTreeMap<u64, TownSim>,
    /// Babies born in town.
    pub born: Vec<Newborn>,
    /// The latest happenings, newest last.
    pub news: Vec<String>,
    /// Where the town's families live, where it isn't the world's home for them (by household
    /// id: the lot's id, or none for the household bin). Set in Edit Town.
    #[serde(default)]
    pub homes: BTreeMap<u64, Option<u64>>,
    /// The lots whose houses were bulldozed in Edit Town (by lot id): empty lots now.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub bulldozed: std::collections::BTreeSet<u64>,
    /// Houses put down on empty lots in Edit Town: the lot (by id), and the lot whose house is a
    /// copy of it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placed: BTreeMap<u64, u64>,
    /// Lots whose type was changed in Edit Town (by lot id): residential (none), or the community
    /// venue they are (one of `COMMUNITY_TYPES`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lot_types: BTreeMap<u64, Option<String>>,
    /// Families moved out of their houses to live about town (by household id: not in the bin).
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub townies: std::collections::BTreeSet<u64>,
    /// New families moved to town (`storymoves::new_family`): the town's as the world's own are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<s3bake::HouseholdBaked>,
    /// Who's famous, and how (by Sim id: `celebrity`), the world's own celebrities read in once,
    /// and the last day the town's rose and fell.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fame: BTreeMap<u64, crate::celebrity::Fame>,
    #[serde(default)]
    pub fame_seeded: bool,
    #[serde(default)]
    pub fame_day: u32,
}

/// The kinds of community lot a lot can be made in Edit Town (as the game's Change Lot Type):
/// each name, and the word in a lot's name the town goes by for what's done there.
pub const COMMUNITY_TYPES: [(&str, &str); 7] = [
    ("Park", "park"),
    ("Gym", "gym"),
    ("Library", "library"),
    ("Pool", "pool"),
    ("Art Gallery", "museum"),
    ("Beach", "beach"),
    ("Fishing Spot", "fishing"),
];

/// A lot made residential, or the community venue `kind` (Edit Town's Change Lot Type): its name
/// and the record's keys say so, as the world's own lots' do.
pub fn retype(lot: &mut s3bake::LotInfo, kind: Option<&str>) {
    lot.string_keys.retain(|k| !k.contains("HouseName"));
    // (Away with anything in the name that says what it was.)
    let bare: String = lot
        .internal_name
        .split('_')
        .filter(|p| {
            let p = p.to_ascii_lowercase();
            !(p == "res" || p == "com" || p.contains("empty") || p.contains("residential") || COMMUNITY_TYPES.iter().any(|(_, k)| p.contains(k)))
        })
        .collect::<Vec<_>>()
        .join("_");
    let bare = bare.replace("Empty", "").replace("empty", "");
    lot.internal_name = match kind.and_then(|k| COMMUNITY_TYPES.iter().find(|(n, _)| *n == k)) {
        Some((_, word)) => format!("Com_{word}_{bare}"),
        None => format!("Res_{bare}"),
    };
}

pub(crate) type TownSim = s3sim::residents::ResidentModel;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Newborn {
    pub first: String,
    pub last: String,
    pub female: bool,
    /// The parents' ids.
    pub parents: (u64, u64),
    pub day: u32,
}

impl TownStory {
    pub(crate) fn migrate_models(&mut self) {
        self.residents.import_models(std::mem::take(&mut self.legacy_sims));
    }

    pub(crate) fn canonicalized(mut self) -> Self { self.migrate_models(); self }

    pub(crate) fn model(&self, id: &u64) -> Option<&TownSim> {
        // Read compatibility is needed before loading has imported appearance data.
        self.residents.model(id).or_else(|| self.legacy_sims.get(id))
    }

    /// Whether a town family has been evicted to the household bin. (The world's households
    /// with no home are its townies, out and about: not in the bin.)
    pub fn evicted(&self, h: &s3bake::HouseholdBaked) -> bool {
        self.homes.get(&h.id) == Some(&None) && !self.townies.contains(&h.id)
    }

    /// Where a town family lives now (the lot's id; none: no home, a townie or evicted).
    pub fn home_of(&self, h: &s3bake::HouseholdBaked) -> Option<u64> {
        match self.homes.get(&h.id) {
            Some(l) => *l,
            None => (h.lot_id != 0).then_some(h.lot_id),
        }
    }

    /// One of the world's Sims as the town has them now (older, perhaps); none if they've
    /// passed away.
    pub fn apply(&self, mut s: Sim) -> Option<Sim> {
        match self.model(&s.id) {
            Some(t) if t.dead || t.gone => None,
            Some(t) => {
                if let Some(a) = &t.age {
                    s.age = crate::save::age_from_name(a);
                }
                Some(s)
            }
            None => Some(s),
        }
    }

    fn tell(&mut self, news: String) {
        self.news.push(news);
        if self.news.len() > 30 {
            self.news.remove(0);
        }
    }
}

/// The daily chances of things happening (a family's move: about one every other day or so, as
/// the game's moves' cooldowns of a day or two allow).
const MOVE_CHANCE: f64 = 0.4;
const PAIR_OFF: f64 = 0.008;
const MARRY: f64 = 0.04;
const BABY: f64 = 0.015;
const FIND_JOB: f64 = 0.03;
const PROMOTION: f64 = 0.02;
/// An elder's chance each day, once their old age has run its course, of passing away.
/// Days into old age before a town elder may pass away (a household elder's span is 14–22).
const ELDER_DAYS: f32 = 14.0;

/// A town Sim's record, as the world has them and the town has changed them.
struct Who {
    id: u64,
    name: String,
    household: u64,
    female: bool,
    last: String,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn progress(
    mut commands: Commands,
    clock: Res<GameClock>,
    settings: Res<crate::options::Settings>,
    mut story: ResMut<TownStory>,
    town: Option<Res<crate::premade::TownPremades>>,
    choice: Option<Res<crate::premade::PremadeChoice>>,
    members: Query<(&Sim, &Relationships), With<HouseholdMember>>,
    spawned: Query<(Entity, &Sim, &Visibility), Without<HouseholdMember>>,
    mut notes: ResMut<Notifications>,
    dormant: Res<crate::household::Dormant>,
    (world_info, household): (Option<Res<crate::loading::CurrentWorld>>, Option<Res<crate::interact::Household>>),
) {
    let Some(town) = town else { return };
    // (Not till the household's here: a save's days caught up on as it comes in would take them
    // for one of the town's families.)
    if members.is_empty() {
        return;
    }
    let today = clock.day();
    if !story.day_initialized {
        story.day = today;
        story.day_initialized = true;
        return;
    }
    if today <= story.day || clock.hour_f() < 6.0 {
        return;
    }
    // Bound the batch without discarding older days. Each subsequent tick can
    // process another day, and this checkpoint is already part of saved TownStory.
    let days = (today - story.day).min(1);
    story.day += days;
    let processed_day = story.day;
    // Who the household knows: any of the town's Sims about with whom one of them has a
    // relationship.
    // (Nor those of the households played before: they're as they were left.)
    let household_ids: Vec<u64> = members.iter().map(|(s, _)| s.id).chain(dormant.member_ids()).collect();
    let mut known: Vec<u64> = Vec::new();
    for (e, s, _) in &spawned {
        if members.iter().any(|(_, r)| r.0.contains_key(&e)) {
            known.push(s.id);
        }
    }
    let playing = choice.as_ref().map(|c| c.0.id);
    let world: Vec<(Who, &s3formats::premade::PremadeSim)> = town
        .0
        .households
        .iter()
        .filter(|h| Some(h.id) != playing && !h.name.to_ascii_lowercase().contains("ghost"))
        .flat_map(|h| h.members.iter().map(move |m| (h.id, m)))
        .filter(|(_, m)| !m.first_name.is_empty() && !household_ids.contains(&m.id))
        .map(|(hid, m)| {
            let s = crate::premade::to_sim(m);
            (Who { id: m.id, name: s.full_name(), household: hid, female: m.female, last: s.last.clone() }, m)
        })
        .collect();
    let mut rng = rand::rng();
    let per_day = 1.0 / settings.lifespan.factor();
    let mut died: Vec<u64> = Vec::new();
    for _ in 0..days {
        // Everyone's record, from the world's to begin with (each at some point in their life
        // stage, so the town doesn't all have its birthdays at once).
        for (w, m) in &world {
            if story.residents.model(&w.id).is_none() {
                // (A stage with no end, like an elder's, counts as a month or so.)
                let span = crate::aging::stage_days(crate::premade::age_of(m.age));
                let span = if span.is_finite() { span } else { ELDER_DAYS };
                let t = TownSim {
                    partner: m.partner,
                    spouse: m.spouse,
                    // (The world file names the career by its class: ours by its name.)
                    job: m.career.as_ref().map(|(c, l)| (track(c).map_or_else(|| c.clone(), |t| t.name.to_string()), (*l).max(1) as u32)),
                    days: rng.random_range(0.0..span.max(1.0) * 0.6).floor(),
                    ..default()
                };
                story.residents.model_or_insert_with(w.id, || t);
            }
        }
        let age_of = |story: &TownStory, w: &Who, m: &s3formats::premade::PremadeSim| {
            story.residents.model(&w.id).and_then(|t| t.age.as_deref()).map_or_else(|| crate::premade::age_of(m.age), crate::save::age_from_name)
        };
        // Growing older.
        if settings.aging {
            for (w, m) in &world {
                let age = age_of(&story, w, m);
                let marathon = story.residents.records.get(&w.id).and_then(|r| r.state.as_ref()).is_some_and(|s| crate::journal::earned(Some(&s.journal), "Marathon Runner"));
                let Some(t) = story.residents.model_mut(&w.id) else { continue };
                if !t.present() {
                    continue;
                }
                t.days += per_day;
                if age == Age::Elder {
                    let mut survival = crate::aging::Aging { days: t.days, elder_span: t.elder_span, elder_risk: t.elder_risk };
                    let dead = survival.old_age_today(settings.lifespan.elder_mortality() * if marathon { 0.75 } else { 1.0 });
                    t.elder_risk = survival.elder_risk;
                    if dead {
                        t.dead = true;
                        died.push(w.id);
                        let spouse = t.spouse;
                        story.tell(format!("{} has passed away peacefully of old age.", w.name));
                        if let Some(s) = spouse.and_then(|s| story.residents.model_mut(&s)) {
                            s.spouse = None;
                        }
                    }
                } else if crate::aging::remaining_days(crate::aging::stage_days(age), t.days, settings.lifespan.factor()) == 0
                    && let Some(next) = crate::aging::next_age(age)
                {
                    t.days = 0.0;
                    t.age = Some(crate::save::age_name(next).to_string());
                    story.tell(format!("{} is now {}.", w.name, crate::aging::age_word(next)));
                }
            }
        }
        // (With the options' Story Progression off, the town only grows older.)
        if !settings.story_progression {
            continue;
        }
        let grown = |story: &TownStory, w: &Who, m: &s3formats::premade::PremadeSim| matches!(age_of(story, w, m), Age::YoungAdult | Age::Adult);
        let alive = |story: &TownStory, id: u64| world.iter().any(|(w, _)| w.id == id) && story.residents.model(&id).is_some_and(|t| t.present());
        // Couples marry.
        for (w, _) in &world {
            let Some(t) = story.residents.model(&w.id) else { continue };
            if !t.present() || t.spouse.is_some() {
                continue;
            }
            let Some(p) = t.partner.filter(|p| alive(&story, *p) && *p > w.id) else { continue };
            if rng.random_bool(MARRY) {
                let other = world.iter().find(|(o, _)| o.id == p).map(|(o, _)| o.name.clone()).unwrap_or_default();
                for (a, b) in [(w.id, p), (p, w.id)] {
                    if let Some(s) = story.residents.model_mut(&a) {
                        s.spouse = Some(b);
                        s.partner = None;
                    }
                }
                story.tell(format!("{} and {other} got married!", w.name));
            }
        }
        // Singles pair off.
        let mut singles: Vec<&(Who, &s3formats::premade::PremadeSim)> = world
            .iter()
            .filter(|(w, m)| grown(&story, w, m) && story.residents.model(&w.id).is_some_and(|t| t.present() && t.partner.is_none() && t.spouse.is_none()))
            .collect();
        singles.shuffle(&mut rng);
        let mut taken: Vec<u64> = Vec::new();
        for a in &singles {
            if taken.contains(&a.0.id) || !rng.random_bool(PAIR_OFF) {
                continue;
            }
            let opposite = rng.random_bool(0.85);
            let Some(b) = singles
                .iter()
                .filter(|b| b.0.id != a.0.id && b.0.household != a.0.household && !taken.contains(&b.0.id) && (b.0.female != a.0.female) == opposite)
                .collect::<Vec<_>>()
                .choose(&mut rng)
                .copied()
            else {
                continue;
            };
            taken.extend([a.0.id, b.0.id]);
            for (x, y) in [(a.0.id, b.0.id), (b.0.id, a.0.id)] {
                if let Some(s) = story.residents.model_mut(&x) {
                    s.partner = Some(y);
                }
            }
            story.tell(format!("{} and {} are going steady.", a.0.name, b.0.name));
        }
        // Married couples have babies.
        for (w, m) in &world {
            if !w.female || !grown(&story, w, m) {
                continue;
            }
            let Some(spouse) = story.residents.model(&w.id).filter(|t| t.present()).and_then(|t| t.spouse) else { continue };
            let kids = m.children.len() + story.born.iter().filter(|b| b.parents.0 == w.id).count();
            if !alive(&story, spouse) || kids >= 3 || !rng.random_bool(BABY) {
                continue;
            }
            let female = rng.random_bool(0.5);
            let baby = crate::sim::random_sim(&mut rng, &w.last, Some(female), Age::Baby);
            let father = world.iter().find(|(o, _)| o.id == spouse).map(|(o, _)| o.name.clone()).unwrap_or_default();
            story.tell(format!("{} and {father} had a baby {}, {}.", w.name, if female { "girl" } else { "boy" }, baby.first));
            story.born.push(Newborn { first: baby.first, last: w.last.clone(), female, parents: (w.id, spouse), day: processed_day });
        }
        // Jobs and promotions.
        for (w, m) in &world {
            if !grown(&story, w, m) {
                continue;
            }
            let Some(t) = story.residents.model_mut(&w.id).filter(|t| t.present()) else { continue };
            match &mut t.job {
                None => {
                    if rng.random_bool(FIND_JOB)
                        && let Some(c) = crate::careers::careers().iter().filter(|c| !c.part_time).collect::<Vec<_>>().choose(&mut rng)
                    {
                        t.job = Some((c.name.to_string(), 1));
                        let news = format!("{} joined the {} career.", w.name, c.name);
                        story.tell(news);
                    }
                }
                Some((career, level)) => {
                    // (Only careers the game has: the news needs the job's title.)
                    let Some(c) = track(career) else { continue };
                    if (*level as usize) < c.levels().len() && rng.random_bool(PROMOTION) {
                        *level += 1;
                        let title = c.levels()[*level as usize - 1].title;
                        let news = format!("{} was promoted to {title}.", w.name);
                        story.tell(news);
                    }
                }
            }
        }
    }
    // Families move house (the game's demographics: about half the houses lived in).
    let mut moved_away: Vec<u64> = Vec::new();
    if settings.story_progression
        && let Some(w) = world_info.as_deref()
    {
        for _ in 0..days {
            if !rng.random_bool(MOVE_CHANCE) && std::env::var_os("STORY_NEW").is_none() {
                continue;
            }
            let active = household.as_deref().map(|h| h.lot_index);
            let occupied = crate::edittown::occupants(&w.data, Some(&town.0), &story, &dormant.0, active);
            let lots = &w.data.lots;
            // (Every residential lot, as the game counts them, built on or not.)
            let houses: Vec<crate::storymoves::House> = (0..lots.len())
                .filter(|&i| lots[i].is_residential())
                .map(|i| {
                    let b = w.data.buildings.get(&i);
                    crate::storymoves::House { lot: i, lived_in: occupied.contains_key(&i), furnished: b.is_some_and(|b| b.is_furnished()), beds: b.map_or(0, crate::storymoves::beds) }
                })
                .collect();
            // (Not the household's own, nor those played before, nor the world's ghosts.)
            let families: Vec<crate::storymoves::Family> = town
                .0
                .households
                .iter()
                .filter(|h| Some(h.id) != playing && crate::edittown::town_family(h, &story) && !h.members.iter().any(|m| household_ids.contains(&m.id)))
                .map(|h| crate::storymoves::Family {
                    id: h.id,
                    home: story.home_of(h).and_then(|id| lots.iter().position(|l| l.id == id)).filter(|_| !story.evicted(h)),
                    size: h.members.iter().filter(|m| !m.first_name.is_empty() && story.apply(crate::premade::to_sim(m)).is_some()).count(),
                    known: h.members.iter().any(|m| known.contains(&m.id)),
                })
                // (The household bin's are Edit Town's to place; whoever's at the household's home
                // is the household.)
                .filter(|f| f.home.is_some() || !story.homes.contains_key(&f.id) || story.townies.contains(&f.id))
                .filter(|f| f.home.is_none() || f.home != active)
                .collect();
            debug!("story moves: {} of {} houses lived in, {} furnished empty, {} families", houses.iter().filter(|h| h.lived_in).count(), houses.len(), houses.iter().filter(|h| !h.lived_in && h.furnished).count(), families.len());
            // (STORY_NEW=1, tests: a new family into the first empty furnished house.)
            let forced = std::env::var_os("STORY_NEW").is_some() && story.created.is_empty();
            let forced = forced.then(|| houses.iter().find(|h| !h.lived_in && h.furnished && h.beds > 0).map(|h| crate::storymoves::Move::New { lot: h.lot, size: h.beds.min(4) })).flatten();
            let Some(m) = forced.or_else(|| crate::storymoves::choose(&houses, &families, &mut rng)) else { continue };
            // (A new family: made, kept, and the town's from now on.)
            if let crate::storymoves::Move::New { lot, size } = m {
                let taken: Vec<String> = town.0.households.iter().map(|h| h.name.clone()).chain(story.created.iter().map(|h| h.name.clone())).collect();
                let mut f = crate::storymoves::new_family(size, &taken, &mut rng);
                f.lot_id = lots[lot].id;
                let news = format!("The {} family is the latest to move into town.", f.name);
                info!("story progression: {news} (household {})", f.id);
                story.homes.insert(f.id, Some(lots[lot].id));
                story.created.push(f);
                story.tell(news);
                continue;
            }
            let id = match m {
                crate::storymoves::Move::In { family, .. } | crate::storymoves::Move::Out { family } | crate::storymoves::Move::Away { family } | crate::storymoves::Move::House { family, .. } => family,
                crate::storymoves::Move::New { .. } => continue,
            };
            let Some(h) = town.0.households.iter().find(|h| h.id == id) else { continue };
            let known_family = families.iter().any(|f| f.id == id && f.known);
            let (news, word) = match m {
                crate::storymoves::Move::In { lot, .. } => {
                    story.homes.insert(id, Some(lots[lot].id));
                    story.townies.remove(&id);
                    (format!("The {} family is the latest to move into a home in town.", h.name), None)
                }
                crate::storymoves::Move::Out { .. } => {
                    story.homes.insert(id, None);
                    story.townies.insert(id);
                    (format!("The {} household has moved out of their house.", h.name), None)
                }
                crate::storymoves::Move::Away { .. } => {
                    story.homes.insert(id, None);
                    story.townies.remove(&id);
                    for m in &h.members {
                        story.residents.model_or_insert_with(m.id, TownSim::default).gone = true;
                        moved_away.push(m.id);
                    }
                    (
                        format!("The {} household recently moved away from town.", h.name),
                        Some(format!("The {} family has decided to move to another town. Hopefully you didn't loan them any books or money!", h.name)),
                    )
                }
                crate::storymoves::Move::House { lot, bigger, .. } => {
                    story.homes.insert(id, Some(lots[lot].id));
                    (format!("The {} family moved to a {} house.", h.name, if bigger { "bigger" } else { "smaller" }), None)
                }
                crate::storymoves::Move::New { .. } => continue,
            };
            info!("story progression: {news} (household {id})");
            if known_family {
                notes.push(word.unwrap_or_else(|| news.clone()));
            }
            story.tell(news);
        }
    }
    // Those the household knows: a word; and they look their age if they're about.
    let fresh: Vec<String> = story.news.iter().rev().take(12).cloned().collect();
    for n in fresh.iter().rev() {
        if world.iter().any(|(w, _)| known.contains(&w.id) && n.starts_with(&w.name)) {
            notes.push(n.clone());
        }
    }
    for (e, s, vis) in &spawned {
        // (Out of sight, they're gone.)
        if (died.contains(&s.id) || moved_away.contains(&s.id)) && *vis == Visibility::Hidden {
            commands.entity(e).try_despawn();
        }
    }
    let active: std::collections::HashSet<_> = members.iter().map(|(s, _)| s.id)
        .chain(dormant.member_ids()).chain(spawned.iter().map(|(_, s, _)| s.id)).collect();
    crate::residents::sync_background(&mut story, &active, clock.minutes);
    info!("story progression: {days} day(s), {} news", story.news.len());
}

/// The town's new families among its own (a save's, as it comes in; a new one, as it moves in).
fn merge_created(story: Res<TownStory>, town: Option<ResMut<crate::premade::TownPremades>>) {
    let Some(mut town) = town else { return };
    if !story.created.iter().any(|f| !town.0.households.iter().any(|h| h.id == f.id)) {
        return;
    }
    let all = std::sync::Arc::make_mut(&mut town.0);
    for f in &story.created {
        if !all.households.iter().any(|h| h.id == f.id) {
            all.households.push(f.clone());
        }
    }
}

/// Reading the paper: the town's latest news.
#[derive(Component)]
pub struct ReadTheNews;

fn read_the_news(mut commands: Commands, story: Res<TownStory>, sims: Query<(Entity, &Sim), With<ReadTheNews>>, mut notes: ResMut<Notifications>) {
    for (e, sim) in &sims {
        commands.entity(e).remove::<ReadTheNews>();
        let news: Vec<&String> = story.news.iter().rev().take(3).collect();
        if news.is_empty() {
            notes.push(format!("{} read the paper. Nothing much has happened in town.", sim.first));
        } else {
            let list: Vec<&str> = news.iter().map(|s| s.as_str()).collect();
            notes.push(format!("{} read the paper. In the news: {}", sim.first, list.join(" ")));
        }
    }
}

/// A career by its name, or by the class the world files name it by (`LawEnforcement`).
pub(crate) fn track(name: &str) -> Option<&'static crate::careers::CareerTrack> {
    let all = crate::careers::careers();
    all.iter().find(|c| c.name == name).or_else(|| crate::premade::career_of(name).and_then(|i| all.get(i)))
}

#[cfg(test)]
mod lot_type_tests {
    use super::*;

    fn lot(name: &str, keys: &[&str]) -> s3bake::LotInfo {
        s3bake::LotInfo { id: 1, internal_name: name.into(), corner: [0.0; 3], rotation: 0.0, width: 30, depth: 40, string_keys: keys.iter().map(|k| k.to_string()).collect() }
    }

    #[test]
    fn retyped_lots_read_as_their_new_type() {
        // A house made a park: no longer residential, and a park to the town's venues.
        let mut l = lot("15MaywSubNoSim", &["World/SV/HouseName:Capitola"]);
        assert!(l.is_residential());
        retype(&mut l, Some("Park"));
        assert!(!l.is_residential());
        assert!(!crate::rabbitholes::activities(&l).is_empty());
        // An empty lot made a gym, then back to a home.
        let mut e = lot("55WatrLLnPviewEmpty", &[]);
        retype(&mut e, Some("Gym"));
        assert!(!e.is_residential() && e.internal_name.to_ascii_lowercase().contains("gym"), "{}", e.internal_name);
        retype(&mut e, None);
        assert!(e.is_residential() && !e.internal_name.to_ascii_lowercase().contains("gym"), "{}", e.internal_name);
        // A park made a home.
        let mut p = lot("Com_suburbanPark_30x30", &[]);
        retype(&mut p, None);
        assert!(p.is_residential(), "{}", p.internal_name);
        assert!(crate::rabbitholes::activities(&p).is_empty(), "{}", p.internal_name);
    }
}
