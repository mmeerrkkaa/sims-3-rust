# Sims 3 Rust

A project to bring **The Sims 3 to a new high-performance engine written in Rust**.
The goal is to preserve the game's open world and life simulation while improving
performance, responsiveness, and stability.

Developed with **OpenAI Codex**, building on [Eamo5's original implementation](https://github.com/Eamo5/sims-3-bevy).
This repository is the home of our version and its future development.

## Status

The port is under active development. Existing content loaders and gameplay are being
migrated incrementally to the new architecture; the game is not yet feature-complete.

Current engine work includes multiple active lots, persistent resident identities,
saved guest/resident roles and departure deadlines, fixed-step needs/actions/movement,
bounded catch-up, saved simulation time debt, and human and pet animation clocks driven
by simulation ticks with interpolated poses. Pet needs, roaming, greetings, training
and horse riding also use ordered simulation phases.
Remaining gameplay systems, navigation and background residents
are still being migrated. Performance gains are measured as each subsystem changes.

## Build and run

You need Rust, a supported graphics driver, and an installed copy of The Sims 3.
Game assets and the converted cache are not included in this repository.

```powershell
# Convert one world. The first conversion can take a while.
cargo run --release -p s3bake -- --data "D:\Games\The Sims 3" --world "Sunset Valley"

# Start the game.
cargo run --release -p sims3 -- --data "D:\Games\The Sims 3"
```

Use `--all-worlds` instead of `--world` to convert every installed world.
The game can also build a missing cache on its first run.

| Setting | Purpose | Default |
| --- | --- | --- |
| `SIMS3_DATA` | Original game installation | Set for your machine |
| `SIMS3_CACHE` | Converted content | `baked/` |
| `SIMS3_SAVES` | This project's JSON saves | `saves/` |
| `SIMS3_PERF` | Measurements written to a new CSV | Disabled |
| `SIMS3_PROFILE=1` | Additional system timings | Disabled |

These are this project's saves; original save and script-mod compatibility is not
established. Converter updates can require parts of the cache to be rebuilt.

## Engine architecture

| Crate | Responsibility |
| --- | --- |
| `s3pkg` | Original package access |
| `s3formats` | Content format decoding |
| `s3bake` | Offline conversion and asset cache |
| `s3sim` | Renderer-independent scheduling and persistent simulation time |
| `sims3` | Game application, gameplay integration, rendering, UI and input |

The fixed core processes approximately 30 wall-time steps per second, scaled by game
speed. Catch-up is limited to four steps per frame; remaining work stays as time debt.
This bounds catch-up bursts, not the CPU cost of each step. Sustained overload still
requires reducing work or choosing a cheaper simulation level.

## Port roadmap

1. **Finish the simulation boundary.** Move remaining gameplay and clip timing into
   explicit phases; interpolate presentation between simulation states.
2. **Make resident ownership reliable.** Preserve identity, state and important events
   across active lots, public spaces, travel, background simulation and save/load.
3. **Introduce L0-L3 simulation.** Detailed nearby interaction, reduced updates on other
   active lots, scheduled events for distant residents, and statistical progression
   where appropriate. Moving the camera must not cancel a resident's ongoing task.
4. **Improve navigation and reservations.** Bound requests, version topology, track
   interaction ownership, and handle congestion without cross-lot resets.
5. **Scale the open world.** Work toward a city of 1,000 residents across households
   and public spaces. Measure simulation progress, backlog, memory, CPU cost and frame
   time. This population is a target, not a demonstrated capacity.
6. **Optimize measured bottlenecks.** Add batching, spatial indexes, caching and parallel
   jobs. Consider GPU compute for suitable bulk work after measuring transfer and
   synchronization costs.
7. **Add multiplayer.** Plan shared-world play with synchronized resident state and
   interactions. Multiplayer is a future goal and is not implemented yet.

We preserve working mechanics during migration and validate each subsystem as it moves.
Rust, ECS, fixed ticks, multithreading and GPU compute are tools, not evidence of a
performance improvement on their own.

## Provenance

Upstream Git history and author attribution are retained. This repository is an
independent continuation, not an official EA or Maxis release. The imported upstream
snapshot has no repository-wide license file; no new blanket license is asserted for
upstream code or proprietary game content.
