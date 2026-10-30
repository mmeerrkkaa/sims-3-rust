//! Explicit local-lot simulation stress scenario; never enabled in ordinary play.
use bevy::prelude::*;
use rand::SeedableRng;

pub struct StressPlugin;

#[derive(Component)]
pub struct StressSim;

impl Plugin for StressPlugin {
    fn build(&self, app: &mut App) {
        let args: Vec<String> = std::env::args().collect();
        let Some(i) = args.iter().position(|s| s == "--stress-sims") else { return };
        let count = args.get(i + 1).and_then(|s| s.parse::<usize>().ok())
            .filter(|n| (1..=1000).contains(n)).expect("--stress-sims requires 1..1000");
        app.insert_resource(StressCount(count))
            .add_systems(Update, spawn_stress.run_if(in_state(crate::PlayMode::Live)));
    }
}

#[derive(Resource)]
struct StressCount(usize);

fn spawn_stress(
    mut commands: Commands,
    count: Res<StressCount>,
    assets: Res<crate::sim::SimAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    selected: Query<&Transform, With<crate::sim::Selected>>,
    grid: Option<Res<crate::nav::NavGrid>>,
    world: Res<crate::loading::CurrentWorld>,
    mut done: Local<bool>,
) {
    if *done { return; }
    let (Ok(at), Some(grid)) = (selected.single(), grid) else { return };
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5333_5354_5245_5353);
    for i in 0..count.0 {
        let mut sim = crate::sim::random_sim(&mut rng, "Stress", Some(i % 2 == 0), crate::sim::Age::Adult);
        sim.id = u64::MAX - i as u64;
        sim.first = format!("Stress{i}");
        let offset = Vec2::new((i % 25) as f32 * 0.7 - 8.0, (i / 25) as f32 * 0.7);
        let desired = at.translation.xz() + offset;
        let pos = grid.nearest_free(desired).map(|(x,z)| grid.center_of(x,z)).unwrap_or(at.translation.xz());
        let p = Vec3::new(pos.x, world.data.heightmap.sample(pos.x,pos.y), pos.y);
        let e = crate::sim::spawn_sim(&mut commands, &assets, &mut materials, sim, p);
        commands.entity(e).insert((StressSim,
            crate::interact::ActionQueue::default(),
            crate::interact::AutonomyTimer((i % 30) as f32 * 0.1),
            crate::interact::Skills::default(),
            DespawnOnExit(crate::AppState::InGame)));
    }
    info!("stress: spawned {} additional adult Sims with simple articulated bodies and normal autonomy; one shared lot, no jobs/households assigned",count.0);
    *done = true;
}
