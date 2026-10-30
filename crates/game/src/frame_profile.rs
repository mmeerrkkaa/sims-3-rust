//! Opt-in wall timings of main/render schedules. Nested/parallel rows are not additive.
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{BufWriter, Write},
    sync::{Arc, Mutex},
    time::Instant,
};

use bevy::{
    app::{MainScheduleOrder, SubApp},
    ecs::schedule::ScheduleLabel,
    prelude::*,
    render::{
        Render, RenderApp, RenderScheduleOrder, RenderSystems,
        pipelined_rendering::RenderExtractApp,
        renderer::{RenderGraph, RenderGraphSystems},
        view::window::prepare_windows,
    },
};

#[derive(Resource, Clone)]
struct Output(Arc<Mutex<Sink>>);

struct Sink {
    writer: BufWriter<std::fs::File>,
    origin: Instant,
    flushed: Instant,
}

impl Output {
    fn row(&self, domain: &str, frame: u64, stage: &str, start: Instant, end: Instant) {
        let mut sink = self.0.lock().unwrap();
        let elapsed = end.duration_since(sink.origin).as_secs_f64();
        let ms = end.duration_since(start).as_secs_f64() * 1000.0;
        let _ = writeln!(sink.writer, "{elapsed:.6},{domain},{frame},{stage},{ms:.6}");
        if sink.flushed.elapsed().as_secs_f32() >= 1.0 {
            let _ = sink.writer.flush();
            sink.flushed = end;
        }
    }
}

#[derive(Resource, Default)]
struct MainTimer {
    frame: u64,
    begin: Option<Instant>,
    last_end: Option<Instant>,
    starts: HashMap<usize, Instant>,
}

#[derive(ScheduleLabel, Clone, Debug, Hash, PartialEq, Eq)]
struct MainMark(usize, bool);

#[derive(Resource, Default)]
struct RenderTimer {
    frame: u64,
    begin: Option<Instant>,
    mark: Option<Instant>,
}

#[derive(Resource, Default)]
struct GraphTimer(Option<Instant>);

#[derive(Resource, Default)]
struct WindowTimer(Option<Instant>);

#[derive(SystemSet, Clone, Debug, Hash, PartialEq, Eq)]
struct RenderMark(usize);

#[derive(SystemSet, Clone, Debug, Hash, PartialEq, Eq)]
struct GraphMark(usize);

#[derive(SystemSet, Clone, Debug, Hash, PartialEq, Eq)]
struct PrepareMark(usize);

#[derive(Resource, Default)]
struct PrepareTimer(Option<Instant>);

#[derive(Resource, Default)]
struct DetailTimers(HashMap<&'static str, Instant>);

#[derive(ScheduleLabel, Clone, Debug, Hash, PartialEq, Eq)]
struct RenderBegin;

#[derive(ScheduleLabel, Clone, Debug, Hash, PartialEq, Eq)]
struct RenderEnd;

fn wrap_extract(app: &mut SubApp, out: Output, stage: &'static str) {
    if let Some(mut extract) = app.take_extract() {
        app.set_extract(move |main, render| {
            let frame = main.get_resource::<MainTimer>().map_or(0, |t| t.frame);
            let start = Instant::now();
            extract(main, render);
            out.row("main", frame, stage, start, Instant::now());
        });
    }
}

pub(super) fn install(app: &mut App) {
    let Some(path) = std::env::var_os("SIMS3_FRAME_PROFILE") else {
        return;
    };
    let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(e) => {
            error!("cannot create frame profile {path:?}: {e}");
            return;
        }
    };
    let mut writer = BufWriter::new(file);
    let _ = writeln!(writer, "elapsed_s,domain,frame,stage,duration_ms");
    let now = Instant::now();
    let out = Output(Arc::new(Mutex::new(Sink {
        writer,
        origin: now,
        flushed: now,
    })));
    app.insert_resource(out.clone())
        .init_resource::<MainTimer>();
    let stages = [
        ("First", First.intern()),
        ("PreUpdate", PreUpdate.intern()),
        ("FixedLoop", RunFixedMainLoop.intern()),
        ("Update", Update.intern()),
        ("SpawnScene", SpawnScene.intern()),
        ("PostUpdate", PostUpdate.intern()),
        ("Last", Last.intern()),
    ];
    for (i, (name, label)) in stages.into_iter().enumerate() {
        app.add_systems(
            MainMark(i, true),
            move |mut timer: ResMut<MainTimer>, out: Res<Output>| {
                let now = Instant::now();
                if i == 0 {
                    timer.frame += 1;
                    if let Some(end) = timer.last_end {
                        out.row("main", timer.frame, "between_main_frames", end, now);
                    }
                    timer.begin = Some(now);
                }
                timer.starts.insert(i, now);
            },
        );
        app.add_systems(
            MainMark(i, false),
            move |mut timer: ResMut<MainTimer>, out: Res<Output>| {
                let now = Instant::now();
                if let Some(start) = timer.starts.remove(&i) {
                    out.row("main", timer.frame, name, start, now);
                }
                if i == 6 {
                    if let Some(start) = timer.begin {
                        out.row("main", timer.frame, "main_total", start, now);
                    }
                    timer.last_end = Some(now);
                }
            },
        );
        let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
        let at = order.labels.iter().position(|l| *l == label).unwrap();
        order.labels.insert(at, MainMark(i, true).intern());
        order.labels.insert(at + 2, MainMark(i, false).intern());
    }

    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        wrap_extract(render, out.clone(), "sync_and_extract");
        render
            .insert_resource(out.clone())
            .init_resource::<RenderTimer>()
            .init_resource::<GraphTimer>()
            .init_resource::<WindowTimer>();
        render.add_systems(RenderBegin, |mut timer: ResMut<RenderTimer>| {
            timer.frame += 1;
            timer.begin = Some(Instant::now());
        });
        render.add_systems(RenderEnd, |timer: Res<RenderTimer>, out: Res<Output>| {
            if let Some(start) = timer.begin {
                out.row(
                    "render",
                    timer.frame,
                    "render_app_total",
                    start,
                    Instant::now(),
                );
            }
        });
        {
            let mut order = render.world_mut().resource_mut::<RenderScheduleOrder>();
            order.labels.insert(0, RenderBegin.intern());
            order.labels.push(RenderEnd.intern());
        }
        // These phases already run in this order; boundaries add no new phase dependency.
        let phases = [
            RenderSystems::ExtractCommands,
            RenderSystems::PrepareAssets,
            RenderSystems::PrepareMeshes,
            RenderSystems::CreateViews,
            RenderSystems::Specialize,
            RenderSystems::PrepareViews,
            RenderSystems::Queue,
            RenderSystems::PhaseSort,
            RenderSystems::Prepare,
            RenderSystems::Render,
            RenderSystems::Cleanup,
            RenderSystems::PostCleanup,
        ];
        let names = [
            "ExtractCommands",
            "PrepareAssets",
            "PrepareMeshes",
            "CreateViews",
            "Specialize",
            "PrepareViews",
            "Queue",
            "PhaseSort",
            "Prepare",
            "Render",
            "Cleanup",
            "PostCleanup",
        ];
        for (i, phase) in phases.into_iter().enumerate() {
            render.configure_sets(Render, (RenderMark(i), phase, RenderMark(i + 1)).chain());
        }
        for i in 0..=names.len() {
            render.add_systems(
                Render,
                (move |mut timer: ResMut<RenderTimer>, out: Res<Output>| {
                    let now = Instant::now();
                    if i > 0
                        && let Some(start) = timer.mark
                    {
                        out.row("render", timer.frame, names[i - 1], start, now);
                    }
                    timer.mark = Some(now);
                })
                .in_set(RenderMark(i)),
            );
        }
        render
            .init_resource::<PrepareTimer>()
            .init_resource::<DetailTimers>();
        let subphases = [
            RenderSystems::PrepareResources,
            RenderSystems::PrepareResourcesBatchPhases,
            RenderSystems::PrepareResourcesWritePhaseBuffers,
            RenderSystems::PrepareResourcesCollectPhaseBuffers,
            RenderSystems::PrepareResourcesFlush,
            RenderSystems::PrepareBindGroups,
        ];
        let names = [
            "prepare_resources",
            "batch_phases",
            "write_phase_buffers",
            "collect_phase_buffers",
            "flush_resources",
            "bind_groups",
        ];
        for (i, phase) in subphases.into_iter().enumerate() {
            render.configure_sets(Render, (PrepareMark(i), phase, PrepareMark(i + 1)).chain());
            render.configure_sets(Render, PrepareMark(i).in_set(RenderSystems::Prepare));
        }
        render.configure_sets(
            Render,
            PrepareMark(names.len()).in_set(RenderSystems::Prepare),
        );
        for i in 0..=names.len() {
            render.add_systems(
                Render,
                (move |mut timer: ResMut<PrepareTimer>,
                       frame: Res<RenderTimer>,
                       out: Res<Output>| {
                    let now = Instant::now();
                    if i > 0
                        && let Some(start) = timer.0
                    {
                        out.row("render", frame.frame, names[i - 1], start, now);
                    }
                    timer.0 = Some(now);
                })
                .in_set(PrepareMark(i)),
            );
        }
        render.add_systems(Render, (|buffers: Option<Res<bevy::render::batching::gpu_preprocessing::BinUnpackingBuffers>>, frame: Res<RenderTimer>| {
            if frame.frame % 180 != 0 { return; }
            if let Some(buffers) = buffers {
                let jobs: usize = buffers.view_phase_buffers.values().map(|b| b.indexed_unpacking_jobs.len() + b.non_indexed_unpacking_jobs.len()).sum();
                info!("frame profile: {} bin-unpacking jobs, {} metadata records, {} view/phase buffers", jobs, buffers.bin_unpacking_metadata.len(), buffers.view_phase_buffers.len());
            }
        }).after(RenderMark(9)).before(RenderMark(10)));
        // Brackets can include parallel siblings; interpret with the enclosing phase.
        macro_rules! bracket {
            ($system:path, $name:literal, $phase:expr) => {
                render.add_systems(
                    Render,
                    (|mut timer: ResMut<DetailTimers>| {
                        timer.0.insert($name, Instant::now());
                    })
                    .before($system)
                    .in_set($phase),
                );
                render.add_systems(
                    Render,
                    (|timer: Res<DetailTimers>, frame: Res<RenderTimer>, out: Res<Output>| {
                        if let Some(start) = timer.0.get($name) {
                            out.row("render", frame.frame, $name, *start, Instant::now());
                        }
                    })
                    .after($system)
                    .in_set($phase),
                );
            };
        }
        bracket!(
            bevy::pbr::prepare_skins,
            "skins_bracket",
            RenderSystems::PrepareResources
        );
        bracket!(
            bevy::pbr::prepare_mesh_bind_groups,
            "mesh_bind_groups_bracket",
            RenderSystems::PrepareBindGroups
        );
        bracket!(
            bevy::pbr::prepare_mesh_view_bind_groups,
            "mesh_view_bind_groups_bracket",
            RenderSystems::PrepareBindGroups
        );
        bracket!(
            bevy::ui_render::prepare_uinodes,
            "ui_nodes_bracket",
            RenderSystems::PrepareBindGroups
        );
        render.add_systems(
            Render,
            (|mut timer: ResMut<WindowTimer>| timer.0 = Some(Instant::now()))
                .before(prepare_windows)
                .in_set(RenderSystems::PrepareViews),
        );
        render.add_systems(
            Render,
            (|timer: Res<WindowTimer>, frame: Res<RenderTimer>, out: Res<Output>| {
                if let Some(start) = timer.0 {
                    out.row(
                        "render",
                        frame.frame,
                        "window_acquire_bracket",
                        start,
                        Instant::now(),
                    );
                }
            })
            .after(prepare_windows)
            .in_set(RenderSystems::PrepareViews),
        );

        let graph = [
            RenderGraphSystems::Begin,
            RenderGraphSystems::Render,
            RenderGraphSystems::Submit,
            RenderGraphSystems::Finish,
        ];
        let names = [
            "graph_begin",
            "graph_render_cpu",
            "graph_submit",
            "graph_finish",
        ];
        for (i, phase) in graph.into_iter().enumerate() {
            render.configure_sets(RenderGraph, (GraphMark(i), phase, GraphMark(i + 1)).chain());
        }
        for i in 0..=names.len() {
            render.add_systems(
                RenderGraph,
                (move |mut timer: ResMut<GraphTimer>,
                       frame: Res<RenderTimer>,
                       out: Res<Output>| {
                    let now = Instant::now();
                    if i > 0
                        && let Some(start) = timer.0
                    {
                        out.row("render", frame.frame, names[i - 1], start, now);
                    }
                    timer.0 = Some(now);
                })
                .in_set(GraphMark(i)),
            );
        }
    }
    if let Some(extract) = app.get_sub_app_mut(RenderExtractApp) {
        wrap_extract(extract, out, "render_wait_and_extract");
    }
    info!("full frame wall profile enabled: {path:?}; nested/parallel rows are not additive");
}
