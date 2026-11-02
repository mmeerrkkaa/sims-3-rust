//! Opt-in CSV measurements. With SIMS3_PERF unset, no instrumentation is installed.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use bevy::window::PresentMode;

use crate::clock::GameClock;
use crate::perf_metrics::Samples;
use crate::{AppState, PlayMode};

pub struct PerfPlugin;

#[path = "frame_profile.rs"]
mod frame_profile;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct BeforeUpdate;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct AfterUpdate;

fn csv_file(path: &Path, header: &str) -> std::io::Result<BufWriter<File>> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut writer = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    writeln!(writer, "{header}")?;
    writer.flush()?;
    Ok(writer)
}

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        frame_profile::install(app);
        let Some(path) = std::env::var_os("SIMS3_PERF") else {
            return;
        };
        let path = Path::new(&path);
        let writer = csv_file(
            path,
            "elapsed_s,state,mode,speed,sims,objects,frames,window_s,fps,frame_mean_ms,frame_p95_ms,frame_max_ms,update_mean_ms,game_minutes_per_real_s,focused,width,height,present_mode,stress_sims,stress_with_queue,stress_walking,topic_dispatch_last_frame,topic_pending,topic_oldest_delay_ticks,sim_steps_last_frame,sim_debt_real_s,sim_debt_game_min,sim_ticks_total,resident_records,resident_checkpoints,resident_bodies,resident_unloaded_bodies,resident_body_evictions,resident_body_wakes",
        );
        let writer = match writer {
            Ok(writer) => writer,
            Err(e) => {
                error!(
                    "cannot start performance recording at {}: {e}",
                    path.display()
                );
                return;
            }
        };
        let systems = if std::env::var("SIMS3_PROFILE").as_deref() == Ok("1") {
            let profile_path = path.with_extension("systems.csv");
            match csv_file(
                &profile_path,
                "elapsed_s,state,mode,speed,focused,system,calls,total_ms,mean_us,max_us,frames",
            ) {
                Ok(writer) => {
                    crate::perf_scopes::enable();
                    Some(writer)
                }
                Err(e) => {
                    error!(
                        "cannot start system profiling at {}: {e}",
                        profile_path.display()
                    );
                    None
                }
            }
        } else {
            None
        };
        info!("recording performance to {}", path.display());
        let now = Instant::now();
        app.insert_resource(Recording {
            writer: Some(writer),
            systems,
            origin: now,
            update_start: now,
            previous_start: None,
            key: None,
            samples: Samples::default(),
            game_start: 0.0,
        })
        .add_systems(BeforeUpdate, begin_update)
        .add_systems(AfterUpdate, finish_update);
        let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
        order.insert_before(Update, BeforeUpdate);
        order.insert_after(Update, AfterUpdate);
    }
}

#[derive(Resource)]
struct Recording {
    writer: Option<BufWriter<File>>,
    systems: Option<BufWriter<File>>,
    origin: Instant,
    update_start: Instant,
    previous_start: Option<Instant>,
    key: Option<(
        AppState,
        Option<PlayMode>,
        usize,
        bool,
        u32,
        u32,
        PresentMode,
    )>,
    samples: Samples,
    game_start: f64,
}

fn begin_update(mut recording: ResMut<Recording>) {
    recording.update_start = Instant::now();
}

fn finish_update(
    mut recording: ResMut<Recording>,
    state: Res<State<AppState>>,
    mode: Option<Res<State<PlayMode>>>,
    clock: Res<GameClock>,
    sims: Query<(), With<crate::sim::Sim>>,
    objects: Query<(), With<crate::interact::GameObject>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    stress: Query<(&crate::interact::ActionQueue, Option<&crate::nav::PathFollow>), With<crate::population_stress::StressSim>>,
    topics: Res<crate::topics::TopicWorkload>,
    ticks: Res<crate::clock::TickWorkload>,
    story: Res<crate::story::TownStory>,
    (bodies,unloaded,cache):(Query<(),(With<crate::sim::Sim>,With<crate::simbody::Skeleton>)>,Query<(),With<crate::resident_bodies::UnloadedBody>>,Res<crate::resident_bodies::BodyCache>),
) {
    let update_ms = recording.update_start.elapsed().as_secs_f64() * 1000.0;
    let r = &mut *recording;
    if r.writer.is_none() {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let key = (
        state.get().clone(),
        mode.map(|m| *m.get()),
        clock.speed,
        window.focused,
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        window.present_mode,
    );
    let start = r.update_start;
    let previous = r.previous_start.replace(start);
    if r.key.as_ref() != Some(&key) {
        r.key = Some(key.clone());
        r.samples = Samples::default();
        r.game_start = clock.minutes;
        if r.systems.is_some() {
            let _ = crate::perf_scopes::drain();
        }
        return;
    }
    let Some(previous) = previous else { return };
    r.samples.push(
        start.duration_since(previous).as_secs_f64() * 1000.0,
        update_ms,
    );
    if r.samples.seconds() < 1.0 {
        return;
    }
    let s = r.samples.take_summary().unwrap();
    let game_rate = if key.0 == AppState::InGame && key.1 == Some(PlayMode::Live) {
        (clock.minutes - r.game_start) / s.seconds
    } else {
        0.0
    };
    r.game_start = clock.minutes;
    let writer = r.writer.as_mut().unwrap();
    let result = (|| -> std::io::Result<()> {
        writeln!(
            writer,
            "{:.3},{:?},{:?},{},{},{},{},{:.6},{:.3},{:.3},{:.3},{:.3},{:.3},{:.6},{},{},{},{:?},{},{},{},{},{},{},{},{:.6},{:.6},{},{},{},{},{},{},{}",
            start.duration_since(r.origin).as_secs_f64(),
            key.0,
            key.1,
            key.2,
            sims.iter().count(),
            objects.iter().count(),
            s.frames,
            s.seconds,
            s.fps,
            s.mean_ms,
            s.p95_ms,
            s.max_ms,
            s.update_mean_ms,
            game_rate,
            key.3,
            key.4,
            key.5,
            key.6,
            stress.iter().count(),
            stress.iter().filter(|(q,_)| !q.0.is_empty()).count(),
            stress.iter().filter(|(_,p)| p.is_some_and(|p| !p.done)).count(),
            topics.batch, topics.pending, topics.oldest_delay,
            ticks.steps, ticks.pending_seconds, ticks.pending_minutes, clock.timeline.ticks,
            story.residents.records.len(), story.residents.records.values().filter(|r| r.checkpoint_at.is_some()).count(),
            bodies.iter().count(),unloaded.iter().count(),cache.evicted,cache.woken
        )?;
        writer.flush()
    })();
    if let Err(e) = result {
        error!("performance recording stopped: {e}");
        r.writer = None;
    }
    if let Some(writer) = r.systems.as_mut() {
        let result = (|| -> std::io::Result<()> {
            for sample in crate::perf_scopes::drain() {
                writeln!(
                    writer,
                    "{:.3},{:?},{:?},{},{},{},{},{:.6},{:.3},{:.3},{}",
                    start.duration_since(r.origin).as_secs_f64(),
                    key.0,
                    key.1,
                    key.2,
                    key.3,
                    sample.section.name(),
                    sample.calls,
                    sample.nanos as f64 / 1e6,
                    sample.nanos as f64 / sample.calls as f64 / 1000.0,
                    sample.max_nanos as f64 / 1000.0,
                    s.frames
                )?;
            }
            writer.flush()
        })();
        if let Err(e) = result {
            error!("system profiling stopped: {e}");
            r.systems = None;
        }
    }
}
