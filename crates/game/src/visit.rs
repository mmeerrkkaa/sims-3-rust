//! Out on the town: a Sim drives to a community lot (a park, the library, the gym, the beach)
//! where the lot's furniture is there to use, on a walk grid of its own, and drives home again.
//! The rest of the household carries on at home.

use bevy::prelude::*;
use rand::Rng;

use crate::baked::Baked;
use crate::building::ActiveBuilding;
use crate::camera::SimsCamera;
use crate::clock::GameClock;
use crate::interact::Notifications;
use crate::loading::{Catalog, CurrentWorld, WorldInfo};
use crate::nav::{Floor, NavGrid, Obstacle};
use crate::objects::{AssetCtx, ObjectAssets, spawn_parts};
use crate::rabbitholes::{Activity, AtRabbitHole};
use crate::sim::{Selected, Sim};
use crate::{AppState, PlayMode};

pub struct VisitPlugin;

impl Plugin for VisitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OpenLots>().add_systems(OnExit(AppState::InGame), clear_open_lots).add_systems(Update, (arrivals.after(crate::save::apply_loaded_game), off_the_lot, close_lot, rebuild_lot_grid, walls_down, guests).chain().run_if(in_state(PlayMode::Live)).in_set(crate::clock::SimPhase::Gameplay));
        #[cfg(local_validation)]
        if std::env::var_os("OPEN_LOTS_TEST").is_some() {
            app.add_systems(Update, crate::open_lots_test::run.after(rebuild_lot_grid).run_if(in_state(PlayMode::Live)).in_set(crate::clock::SimPhase::Gameplay));
        }
        #[cfg(local_validation)]
        if std::env::var_os("OPEN_LOTS_RELOAD_TEST").is_some() {
            app.add_systems(Update, crate::open_lots_test::verify_restore.after(rebuild_lot_grid).run_if(in_state(PlayMode::Live)).in_set(crate::clock::SimPhase::Gameplay));
        }
        #[cfg(local_validation)]
        if std::env::var_os("NPC_OWNERSHIP_TEST").is_some() {
            app.init_resource::<crate::npc_ownership_test::Probe>()
                .add_systems(Update, crate::npc_ownership_test::run.after(guests).in_set(crate::clock::SimPhase::Gameplay).run_if(in_state(PlayMode::Live)));
        }
    }
}

/// Detailed lots are owned by simulation occupancy, independently of the camera.
#[derive(Resource, Default)]
pub struct OpenLots(pub std::collections::BTreeMap<usize, VisitedLot>);

fn clear_open_lots(mut lots: ResMut<OpenLots>, building: Option<ResMut<ActiveBuilding>>) {
    lots.0.clear();
    if let Some(mut b) = building { b.away.clear(); }
}

/// The drive to or from a community lot (away, like a trip to a rabbit hole).
pub static DRIVE: Activity = crate::rabbitholes::act("Drive", 0.0, 0, [0.0; 6], None);
const DRIVE_MINUTES: f64 = 20.0;
/// How long a lot stays open after the last of the household leaves (game minutes).
const LINGER_MINUTES: f64 = 30.0;

/// A Sim out on a community lot.
#[derive(Component, Clone, Copy)]
pub struct OnLot(pub usize);

/// Driving to a community lot.
#[derive(Component)]
pub struct Trip(pub usize);

#[derive(Component)]
struct ResumeLotPosition {
    position: Vec3,
    yaw: f32,
    floor: u8,
}

/// The persistent model contains stable world IDs, not runtime lot indices.
pub(crate) fn saved_location(world: &WorldInfo, on: Option<&OnLot>, trip: bool, rabbit: Option<&AtRabbitHole>) -> Option<s3sim::residents::WorldLocation> {
    use s3sim::residents::WorldLocation::*;
    if let Some(on) = on {
        return world.lots.get(on.0).map(|l| AtLot { lot: l.id });
    }
    let at = rabbit.filter(|at| std::ptr::eq(at.activity, &DRIVE))?;
    if trip {
        world.lots.get(at.lot).map(|l| ToLot { lot: l.id, arrive_at: s3sim::tick_at(at.inside_from) })
    } else {
        Some(ToHome { arrive_at: s3sim::tick_at(at.until) })
    }
}

pub(crate) fn restore_location(ec: &mut bevy::ecs::system::EntityCommands, world: &WorldInfo, saved: &crate::save::SavedSim, now: f64) {
    use s3sim::residents::WorldLocation::*;
    let Some(location) = saved.world_location else { return };
    let (destination, arrival) = match location {
        AtLot { lot } => (Some(lot), now),
        ToLot { lot, arrive_at } => (Some(lot), arrive_at as f64 / 10.0),
        ToHome { arrive_at } => {
            ec.remove::<(OnLot, Trip, ResumeLotPosition, crate::interact::OffLot)>()
                .insert((AtRabbitHole { lot: usize::MAX, activity: &DRIVE, inside_from: now, until: arrive_at as f64 / 10.0, place: "home".into() }, Visibility::Hidden));
            return;
        }
    };
    let Some(lot) = world.lots.iter().position(|l| Some(l.id) == destination) else {
        warn!("cannot restore {} on missing lot {:?}", saved.first, destination);
        return;
    };
    ec.remove::<(OnLot, crate::interact::OffLot)>().insert((
        AtRabbitHole { lot, activity: &DRIVE, inside_from: arrival, until: f64::MAX, place: place_name(world, lot) },
        Trip(lot), Visibility::Hidden,
    ));
    if matches!(location, AtLot { .. }) {
        ec.insert(ResumeLotPosition { position: Vec3::from(saved.position), yaw: saved.yaw, floor: saved.floor });
    }
}

/// A townie spending time on the visited lot until `leave_at`.
#[derive(Component)]
pub struct LotGuest {
    pub leave_at: f64,
}

/// How many townies are out on a community lot at once, and when.
const GUESTS: usize = 4;
const GUEST_HOURS: std::ops::Range<f32> = 8.0..22.0;

/// Furniture of the community lot being visited.
#[derive(Component, Clone, Copy)]
pub struct LotObject(pub usize);

/// The community lot the household is out at: its walk grid, its way in and out, and what was
/// spawned for it.
#[derive(Resource)]
pub struct VisitedLot {
    pub lot: usize,
    pub grid: NavGrid,
    /// The visited building's upper floors, walked and reached by its stairs and elevators.
    pub upper: crate::nav::UpperFloors,
    /// Where Sims arrive and leave from, by the road.
    pub exit: Vec2,
    root: Entity,
    objects: Vec<Entity>,
    empty_since: Option<f64>,
    /// Frames to wait for the furniture's transforms before building the grid.
    settle: u8,
    /// Game minute the next townie may turn up.
    next_guest: f64,
}

/// Kinds of furniture that make a lot somewhere to spend time.
const USABLE: [&str; 18] = [
    "Chess", "Bookshelf", "Computer", "Treadmill", "Barbeque", "Picnic", "Bench", "Loveseat", "Sofa", "Easel", "Painting", "Toilet", "Shower", "JungleGym", "SwingSet", "Telescope", "Stereo", "Fishing",
];

/// Whether a lot is somewhere to go (not a home, and with furniture to use).
pub fn visitable(world: &WorldInfo, lot: usize) -> bool {
    let Some(l) = world.lots.get(lot) else { return false };
    !l.is_residential() && world.buildings.get(&lot).is_some_and(|b| b.objects.iter().filter(|o| USABLE.iter().any(|k| o.script.contains(k))).count() >= 2)
}

/// Where Sims pull up at a lot: the middle of its front, just inside.
pub fn lot_exit(l: &s3formats::world::LotInfo) -> Vec2 {
    let p = crate::home::lot_center(l) + Quat::from_rotation_y(l.rotation) * Vec3::new(0.0, 0.0, -(l.depth as f32) * 0.5 + 1.5);
    p.xz()
}

/// A lot's name for messages (a family's home by the family).
pub fn place_name(world: &WorldInfo, lot: usize) -> String {
    if let Some(h) = crate::homes::home_name(lot) {
        return h;
    }
    world.lots.get(lot).map_or_else(String::new, |l| crate::rabbitholes::lot_title(l, world.lot_names.get(lot).map_or("", |s| s.as_str())))
}

/// Sets off for a community lot (when the Sim reaches the way out).
pub fn drive_to(commands: &mut Commands, clock: &GameClock, e: Entity, sim: &Sim, lot: usize, place: String, notes: &mut Notifications) {
    notes.push(format!("{} headed to {place}.", sim.first));
    commands.entity(e).remove::<OnLot>().insert((
        AtRabbitHole { lot, activity: &DRIVE, inside_from: clock.minutes + DRIVE_MINUTES, until: f64::MAX, place },
        Trip(lot),
        Visibility::Hidden,
    ));
}

/// Teleports to a community lot from the Teleporter's pad: there at once, no drive.
pub fn teleport_to(commands: &mut Commands, clock: &GameClock, e: Entity, sim: &Sim, lot: usize, place: String, notes: &mut Notifications) {
    notes.push(format!("{} teleported to {place}.", sim.first));
    commands
        .entity(e)
        .remove::<OnLot>()
        .insert((AtRabbitHole { lot, activity: &DRIVE, inside_from: clock.minutes, until: f64::MAX, place }, Trip(lot), Visibility::Hidden));
}

/// Drives home from a community lot (from its way out).
pub fn drive_home(commands: &mut Commands, clock: &GameClock, e: Entity, lot: usize, place: String) {
    commands
        .entity(e)
        .remove::<OnLot>()
        .insert((AtRabbitHole { lot, activity: &DRIVE, inside_from: clock.minutes, until: clock.minutes + DRIVE_MINUTES, place }, Visibility::Hidden));
}

/// Opens a community lot: its building for show (walls blocking the way), its ground-floor
/// furniture to use, and a walk grid over it. Returns the lot and its building (for its floors).
fn open_lot(commands: &mut Commands, assets: &mut ObjectAssets, ctx: &mut AssetCtx, catalog: &Catalog, world: &WorldInfo, lot: usize) -> (VisitedLot, Option<ActiveBuilding>) {
    let l = &world.lots[lot];
    let root = commands.spawn((Transform::IDENTITY, Visibility::default(), DespawnOnExit(AppState::InGame))).id();
    let mut objects = Vec::new();
    let mut shown = None;
    if let Some(b) = world.buildings.get(&lot) {
        shown = Some(crate::building::spawn_building(commands, assets, ctx, catalog, b, l, Some(root), true));
        for o in &b.objects {
            if o.script.contains("Stairs") || o.script.contains("Spawner") {
                continue;
            }
            let q = Quat::from_xyzw(o.rotation[0], o.rotation[1], o.rotation[2], o.rotation[3]);
            let q = if q.length_squared() < 1e-6 { Quat::IDENTITY } else { q.normalize() };
            let pos = Vec3::from(o.position);
            // (Each on its floor: upstairs is reached by the stairs and elevators.)
            let Some(s) = crate::home::spawn_game_object_design(commands, assets, ctx, catalog, o.objd, pos, q, o.design) else { continue };
            commands.entity(s.entity).insert((LotObject(lot), Floor(b.object_storey(o))));
            if crate::building::is_opening(&o.script).is_some() || o.script.contains("Column") {
                commands.entity(s.entity).remove::<Obstacle>();
            }
            objects.push(s.entity);
        }
    }
    let center = crate::home::lot_center(l);
    let grid = NavGrid::new(center.xz(), l.width.max(l.depth) as f32 * 0.5 + 8.0);
    (VisitedLot { lot, grid, upper: Default::default(), exit: lot_exit(l), root, objects, empty_since: None, settle: 3, next_guest: 0.0 }, shown)
}

/// Sims reaching a community lot open it without displacing residents of other lots.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn arrivals(
    mut commands: Commands,
    clock: Res<GameClock>,
    world: Res<CurrentWorld>,
    (data, catalog, mut assets): (Res<Baked>, Res<Catalog>, ResMut<ObjectAssets>),
    (mut meshes, mut images, mut mats): (ResMut<Assets<Mesh>>, ResMut<Assets<Image>>, ResMut<Assets<StandardMaterial>>),
    mut visited: ResMut<OpenLots>,
    mut building: Option<ResMut<ActiveBuilding>>,
    mut q: Query<(Entity, &Trip, &AtRabbitHole, &mut Transform, &mut Floor, &Sim, Has<Selected>, Option<&ResumeLotPosition>)>,
    mut cam: Query<&mut SimsCamera>,
    mut notes: ResMut<Notifications>,
) {
    let mut rng = rand::rng();
    for (e, trip, at, mut tf, mut floor, sim, selected, resume) in &mut q {
        if clock.minutes < at.inside_from {
            continue;
        }
        let lot = trip.0;
        let Some(l) = world.data.lots.get(lot) else { continue };
        if !visited.0.contains_key(&lot) {
            let mut ctx = AssetCtx { baked: &data.0, meshes: &mut meshes, images: &mut images, materials: &mut mats };
            let (v, shown) = open_lot(&mut commands, &mut assets, &mut ctx, &catalog, &world.data, lot);
            if let Some(b) = building.as_deref_mut() {
                if let Some(shown) = shown { b.away.insert(lot, shown); }
            }
            visited.0.insert(lot, v);
        }
        let p = lot_exit(l) + Vec2::new(rng.random_range(-1.5..1.5), rng.random_range(-1.0..1.0));
        let y = crate::building::walk_height(&world.data, building.as_deref(), Vec3::new(p.x, 0.0, p.y));
        tf.translation = Vec3::new(p.x, y, p.y);
        floor.0 = 1;
        if let Some(resume) = resume {
            tf.translation = resume.position;
            tf.rotation = Quat::from_rotation_y(resume.yaw);
            floor.0 = resume.floor;
        }
        commands.entity(e).remove::<(AtRabbitHole, Trip, ResumeLotPosition, crate::interact::OffLot)>().insert((OnLot(lot), Visibility::Inherited));
        if selected && let Ok(mut c) = cam.single_mut() {
            c.look_at(tf.translation);
            c.distance = c.distance.min(30.0);
        }
        notes.push(format!("{} arrived at {}.", sim.first, at.place));
    }
}

impl VisitedLot {
    /// What the lot's building is drawn under.
    pub fn root(&self) -> Entity {
        self.root
    }
}

fn despawn_lot(commands: &mut Commands, v: &VisitedLot) {
    commands.entity(v.root).try_despawn();
    for &e in &v.objects {
        commands.entity(e).try_despawn();
    }
}

/// Sims who leave a community lot for work or a rabbit hole are no longer on it.
fn off_the_lot(mut commands: Commands, q: Query<Entity, (With<OnLot>, Or<(With<crate::interact::AtWork>, With<AtRabbitHole>)>)>) {
    for e in &q {
        commands.entity(e).remove::<OnLot>();
    }
}

/// A lot nobody's at (or on the way to) closes after a while.
fn close_lot(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut visited: ResMut<OpenLots>,
    mut building: Option<ResMut<ActiveBuilding>>,
    on_lot: Query<&OnLot, With<crate::sim::HouseholdMember>>,
    trips: Query<&Trip>,
) {
    let mut closed = Vec::new();
    for v in visited.0.values_mut() {
    let busy = on_lot.iter().any(|o| o.0 == v.lot) || trips.iter().any(|t| t.0 == v.lot);
    if busy {
        v.empty_since = None;
        continue;
    }
    let since = *v.empty_since.get_or_insert(clock.minutes);
    if clock.minutes - since > LINGER_MINUTES {
        despawn_lot(&mut commands, &v);
        if let Some(b) = building.as_deref_mut() {
            b.away.remove(&v.lot);
        }
        closed.push(v.lot);
    }
    }
    for lot in closed { visited.0.remove(&lot); }
}

/// Townies out on the town: while a community lot is open in the daytime a few turn up (from
/// those not strolling past the house) and spend a while there, using it like anyone else; they
/// go when their time's up or the lot closes.
#[allow(clippy::type_complexity)]
fn guests(
    mut commands: Commands,
    clock: Res<GameClock>,
    world: Res<CurrentWorld>,
    building: Option<Res<ActiveBuilding>>,
    mut visited: ResMut<OpenLots>,
    mut townies: Query<(Entity, &mut Transform, &mut Floor, &crate::town::Townie), (Without<LotGuest>, Without<crate::interact::Invited>, Without<crate::interact::Visitor>, Without<crate::sim::HouseholdMember>)>,
    out: Query<(Entity, &LotGuest, &OnLot), Without<crate::town::Townie>>,
) {
    let mut rng = rand::rng();
    // Time to go (or the lot closed): back to strolling.
    let mut here = std::collections::HashMap::<usize, usize>::new();
    for (e, g, on) in &out {
        if visited.0.contains_key(&on.0) && clock.minutes < g.leave_at {
            *here.entry(on.0).or_default() += 1;
            continue;
        }
        // Claims are released by the transition before the queue is dropped.
        crate::town::retire_to_background(&mut commands, e, clock.minutes + rng.random_range(30.0..120.0));
    }
    let hour = clock.hour_f();
    let mut assigned = std::collections::HashSet::new();
    for v in visited.0.values_mut() {
    if here.get(&v.lot).copied().unwrap_or(0) >= GUESTS || !GUEST_HOURS.contains(&hour) || clock.minutes < v.next_guest || v.grid.dirty {
        continue;
    }
    v.next_guest = clock.minutes + rng.random_range(10.0..45.0);
    let free: Vec<Entity> = townies.iter().filter(|t| !t.3.walking && !assigned.contains(&t.0)).map(|t| t.0).collect();
    if free.is_empty() {
        continue;
    }
    let pick = free[rng.random_range(0..free.len())];
    assigned.insert(pick);
    let Ok((e, mut tf, mut floor, _)) = townies.get_mut(pick) else { continue };
    let p = v.exit + Vec2::new(rng.random_range(-2.0..2.0), rng.random_range(-1.0..1.0));
    tf.translation = Vec3::new(p.x, crate::building::walk_height(&world.data, building.as_deref(), Vec3::new(p.x, 0.0, p.y)), p.y);
    floor.0 = 1;
    commands
        .entity(e)
        .remove::<(crate::town::Townie, crate::nav::PathFollow, crate::interact::OffLot)>()
        .insert((LotGuest { leave_at: clock.minutes + rng.random_range(90.0..240.0) }, OnLot(v.lot), Visibility::Inherited))
        .insert_if_new((crate::interact::ActionQueue::default(), crate::interact::AutonomyTimer(rng.random_range(0.5..3.0)), crate::interact::Skills::default()));
    }
}

/// Up close, the visited lot's walls are shown cut down so Sims inside can be seen.
fn walls_down(cams: Query<&SimsCamera>, mut walls: Query<(&crate::building::VisitWalls, &mut Visibility)>) {
    let Ok(cam) = cams.single() else { return };
    let close = cam.distance < 42.0;
    for (w, mut vis) in &mut walls {
        vis.set_if_neq(if w.cut == close { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// The lot's walk grid, once its furniture has settled in place.
fn rebuild_lot_grid(
    world: Res<CurrentWorld>,
    mut visited: ResMut<OpenLots>,
    obstacles: crate::nav::Obstacles,
    building: Option<Res<ActiveBuilding>>,
    spirals: Res<crate::nav::SpiralLinks>,
) {
    for v in visited.0.values_mut() {
    if v.settle > 0 {
        v.settle -= 1;
        continue;
    }
    if v.grid.dirty {
        v.grid.dirty = false;
        crate::nav::fill_grid(&mut v.grid, &world.data, &obstacles, building.as_deref().and_then(|b| b.away.get(&v.lot)));
        // (A lot without upper floors has none to wait for.)
        v.upper.made = true;
        // (And its upper floors, from the building shown for it.)
        if let Some(a) = building.as_deref().and_then(|b| b.away.get(&v.lot)) {
            crate::nav::rebuild_upper(a, &mut v.upper, &obstacles, &spirals);
            debug!(
                "visit {}: upper floors {:?}, links {:?}",
                v.lot,
                v.upper.grids.iter().map(|(l, g)| (*l, g.blocked.iter().filter(|b| !**b).count())).collect::<Vec<_>>(),
                v.upper.stairs.iter().map(|s| (s.level, s.upper, s.bottom, s.top, s.y0, s.y1)).collect::<Vec<_>>()
            );
        }
    }
    }
}
