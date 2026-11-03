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
and horse riding also use ordered simulation phases. Conversation decisions, listening,
seated chatter, facial expressions and idle animations run on simulation ticks.
Weather, body temperature, mood updates and reaction playback also use fixed phases;
frame ingress retains life/social events while paused. Death detection still runs
once per frame, and pending event queues are not yet part of saved games.
Work, pensions, school and rabbit-hole outings now progress on simulation ticks.
Shift accrual is bounded by its start and end; queued questions pause catch-up
before the dialog appears. Road traffic remains a presentation layer, with
pause-aware car-return and curb-wait timers; persistent travel is still planned.
Wishes, dream trees, lifetime goals and passive happiness also progress on ticks;
independent event subscribers retain paused happenings without consuming each other's queues.
Household aging, town story progression, holidays and celebrity progression now use
ordered tick phases, while body rebuilding stays in frame presentation. Town day
catch-up retains pending days; holiday delivery markers are included in saves.
City profiles and personal checkpoints are now stored by stable resident ID separately
from model preparation. Unmet background residents and street walks survive saves;
loading keeps a small background actor cohort. Daily age, career and couple changes share
a canonical resident record; old town records migrate on load. Household takeover projects
that progression into personal state, and contacts bind when ordinary actors arrive.
New ordinary NPC actors restore their personal checkpoint once at their actual arrival
position; invitations and moves retain the live state instead of replaying the checkpoint.
Contacts and guest pickers use resident IDs; requested contacts receive scene actors
through a per-frame budget. Accepted home-invitation times survive saves, while idle acquaintances
can remain without models after loading.
Remote contacts start without a 3D body. Idle off-lot bodies have a bounded cache
and reload for visible activity or portraits. Logical entities and
personal state remain alive while Entity-based gameplay is being migrated.
Accepted outings retain the promised departure and meeting countdown across saves.
City leisure visitors enter occupied community lots through the shared actor budget,
retaining their planned arrival and departure times. Demand follows household occupancy;
moving the camera does not change their ownership.
Active dates and parties retain their participants, deadlines and accumulated results
across saves.
Cold residents have an initial coarse leisure itinerary between home and community lots.
Departures, travel deadlines and returns are persistent and processed with a tick budget;
detailed activation takes over the existing journey. Work and school calendars use
the actual career locations and hours; idle detailed NPCs can rejoin a saved journey
home while retaining logical identity and personal state. Full city simulation tiers
remain planned. Completed coarse school sessions retain grades and homework; worker
attendance is applied once from durable completion events.
Coarse needs keep a saved minute cursor, use shared decay/interaction rates, and
require available beds, food and plumbing for abstract self-care.
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
| `SIMS3_FRAME_PROFILE` | Main/render stage wall timings written to a new CSV | Disabled |
| `SIMS3_INDIRECT=1` | Opt into indirect drawing for world-camera comparisons | Direct drawing |

The world camera uses direct draws with GPU preprocessing. On the measured Sunset
Valley scene, this avoids expensive per-batch indirect-buffer uploads. Indirect
drawing and GPU culling remain available with `SIMS3_INDIRECT=1` for other workloads.
Frame-profile timings include nested and parallel work; do not add all rows together.

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
7. **Bring a Sims 4-style interface to The Sims 3.** Rework the UI and interaction
   flows for this port. This is a future goal and is not implemented yet.
8. **Add multiplayer.** Plan shared-world play with synchronized resident state and
   interactions. Multiplayer is a future goal and is not implemented yet.

We preserve working mechanics during migration and validate each subsystem as it moves.
Rust, ECS, fixed ticks, multithreading and GPU compute are tools, not evidence of a
performance improvement on their own.

## Provenance

Upstream Git history and author attribution are retained. This repository is an
independent continuation, not an official EA or Maxis release. The imported upstream
snapshot has no repository-wide license file; no new blanket license is asserted for
upstream code or proprietary game content.
