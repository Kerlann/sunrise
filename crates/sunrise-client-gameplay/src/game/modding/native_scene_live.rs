//! Living screen scenes (`DrawContext::draw_scene` with entities, particles or a clock):
//! a scene's blocks as `native_scene` draws them, with entities standing in them and
//! particles that live in them, as GuideME's guide scenes and Ponder show. The game keeps,
//! per calling mod and clock id, a small level of each scene's blocks (for the blocks'
//! animate ticks and the particles' collisions), its own particle engine and its entities,
//! prepared for drawing only as `DrawContext::entity` prepares them. Nothing of it reaches
//! the client's level, its particle engine or its sounds. A scene of blocks alone never
//! comes here.
use super::native_scene::{SceneBlock, SceneQuad, face_shade, shaded, valid_scale};
use glam::{DVec3, Quat, Vec3};
use mod_api::{
    GuiEntity, GuiEntityError, SCENE_ANIMATE_PICKS, SCENE_CATCH_UP, SCENE_MAX_ANIMATING,
    SCENE_MAX_ENTITIES, SCENE_MAX_PARTICLES, Scene, SceneClock, SceneError, SceneView,
};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};
use sunrise_client::{
    animate_tick::{AnimateContext, animate_scene},
    hud::HudBatch,
    mesher::Vertex,
    particles::{ParticleEngine, ParticleLevel},
};
use sunrise_core::geometry::Vec3 as Position;
use sunrise_gameplay_common::game::api_values::TryIntoEngine as _;
use sunrise_level::particle_options::ParticleOptions;
use sunrise_level::{chunk::Chunk, world::World};
use sunrise_sim::mobs::Mob;

/// How many scenes the game keeps: a guide page rarely shows more. The one drawn
/// longest ago goes first.
const KEPT: usize = 16;
/// An entity or particle this far out is refused, as a level's border would refuse it;
/// a block this far out draws but has no animate tick.
const FAR: f64 = 30_000_000.0;

/// One scene's kept state.
struct Live {
    /// The mod that drew it, and its clock's id.
    owner: &'static str,
    id: u32,
    /// The hash of the blocks it was made for.
    key: u64,
    /// The entities it was made for.
    entities: Vec<GuiEntity>,
    /// The last tick it ran.
    tick: u64,
    /// The frame it last drew in.
    frame: u64,
    world: World,
    /// The scene's blocks its small level holds: what the animate ticks pick among.
    picks: Vec<[i32; 3]>,
    engine: ParticleEngine,
    /// One per scene entity, in order.
    mobs: Vec<Mob>,
    /// Geometry scratch, kept so a frame allocates none.
    verts: Vec<Vertex>,
}

/// The living scenes the game's drawing keeps, and the frame's animating budget.
#[derive(Default)]
pub struct LiveScenes {
    live: Vec<Live>,
    /// The frame `animating` counts.
    frame: u64,
    /// The scenes (mod, id) that animated in `frame`.
    animating: Vec<(&'static str, u32)>,
}

impl LiveScenes {
    /// Forgets every kept scene: their particles name the old atlas's sprites and their
    /// entities the old resources' types (a resource reload), or the world closed.
    pub fn clear(&mut self) {
        self.live.clear();
        self.animating.clear();
    }
    /// How many scenes are kept.
    #[cfg(any(test, feature = "test-support"))]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
    /// Whether scene (`owner`, `id`) may animate in `frame`: the first
    /// `SCENE_MAX_ANIMATING` scenes a frame do, each as often as it is drawn.
    pub fn admit(&mut self, frame: u64, owner: &'static str, id: u32) -> bool {
        if self.frame != frame {
            self.frame = frame;
            self.animating.clear();
        }
        if self.animating.contains(&(owner, id)) {
            return true;
        }
        if self.animating.len() >= SCENE_MAX_ANIMATING {
            return false;
        }
        self.animating.push((owner, id));
        true
    }

    /// Runs one living-scene draw against this cache: validates `rect`, `scene` and `view`
    /// (including `view.scale`), converts added particles, and validates/prepares
    /// `GuiEntity::Existing` actors before checking the frame's animating budget. Past the
    /// budget it draws the fallback blocks/last state and returns `TooManyScenes` without
    /// preparing `Kind` entities; within the budget a fresh scene reuses the prepared `Existing`
    /// actors and prepares each remaining entity once.
    #[allow(clippy::too_many_arguments)]
    fn drive(
        &mut self,
        frame: u64,
        owner: &'static str,
        rect: [f32; 4],
        blocks: &[SceneBlock<'_>],
        scene: &Scene,
        view: SceneView,
        mut prepare_entity: impl FnMut(&GuiEntity) -> Result<Mob, GuiEntityError>,
        make_live: impl FnOnce(SceneClock, u64, Vec<Mob>) -> Live,
        advance: impl FnOnce(&mut Live, SceneClock, &[(ParticleOptions, [f64; 3], [f64; 3])]),
        draw: impl FnOnce(Option<&mut Live>, SceneClock) -> Result<(), SceneError>,
    ) -> Result<(), SceneError> {
        let clock = check(rect, scene, view)?;
        // Checked whole before anything is kept; an empty list allocates nothing.
        let added = scene
            .particles
            .iter()
            .zip(0u32..)
            .map(|(p, index)| {
                p.options
                    .try_into_engine(&())
                    .map(|o| (o, p.pos, p.velocity))
                    .map_err(|_| SceneError::BadParticle { index })
            })
            .collect::<Result<Vec<(ParticleOptions, _, _)>, _>>()?;
        // The client's `Existing` entities as they are this frame, refused before anything is
        // budgeted or kept.
        let mut existing = Vec::new();
        for (index, e) in (0u32..).zip(&scene.entities) {
            if let GuiEntity::Existing(_) = e.entity {
                let mob =
                    prepare_entity(&e.entity).map_err(|error| entity_refusal(error, index))?;
                existing.push((index as usize, mob));
            }
        }
        let key = scene_key(blocks);
        let at = self
            .live
            .iter()
            .position(|l| l.owner == owner && l.id == clock.id);
        let same = |l: &Live| {
            l.key == key
                && l.entities.len() == scene.entities.len()
                && l.entities
                    .iter()
                    .zip(&scene.entities)
                    .all(|(a, b)| *a == b.entity)
        };
        if !self.admit(frame, owner, clock.id) {
            // Past the budget: the blocks, and the scene's last state when it is this one's.
            let last = at.filter(|&i| same(&self.live[i]));
            match last {
                Some(i) => {
                    let l = &mut self.live[i];
                    l.frame = frame;
                    draw(Some(l), clock)?;
                }
                None => draw(None, clock)?,
            }
            return Err(SceneError::TooManyScenes);
        }
        let fresh = at.is_none_or(|i| !same(&self.live[i]) || clock.tick < self.live[i].tick);
        let i = if fresh {
            let mut prepared_existing = existing.into_iter().peekable();
            let mut mobs = Vec::with_capacity(scene.entities.len());
            for (index, e) in (0u32..).zip(&scene.entities) {
                let mob = if let Some((_, mob)) =
                    prepared_existing.next_if(|&(slot, _)| slot == index as usize)
                {
                    mob
                } else {
                    prepare_entity(&e.entity).map_err(|error| entity_refusal(error, index))?
                };
                mobs.push(mob);
            }
            let made = make_live(clock, key, mobs);
            match at {
                Some(i) => {
                    self.live[i] = made;
                    i
                }
                None => {
                    if self.live.len() == KEPT
                        && let Some(oldest) = (0..KEPT).min_by_key(|&i| self.live[i].frame)
                    {
                        self.live.swap_remove(oldest);
                    }
                    self.live.push(made);
                    self.live.len() - 1
                }
            }
        } else {
            let slot = at.unwrap_or_default();
            let l = &mut self.live[slot];
            for (index, mob) in existing {
                l.mobs[index] = mob;
            }
            slot
        };
        let l = &mut self.live[i];
        l.frame = frame;
        advance(l, clock, &added);
        for (mob, e) in l.mobs.iter_mut().zip(&scene.entities) {
            pose(mob, e, clock);
        }
        draw(Some(l), clock)
    }
}

/// The blocks' hash: a scene whose blocks change starts over.
pub fn scene_key(blocks: &[SceneBlock<'_>]) -> u64 {
    let mut h = DefaultHasher::new();
    for b in blocks {
        (b.pos, b.state.0).hash(&mut h);
    }
    h.finish()
}

/// The seed of scene `id`'s tick `tick`: the same ticks give the same particles.
pub(in crate::game::client_side) fn tick_seed(id: u32, tick: u64) -> i64 {
    let mut h = DefaultHasher::new();
    (0x5343_454e_u32, id, tick).hash(&mut h);
    h.finish() as i64
}

/// The ticks a scene last run at `last` runs to show `tick`: whether it starts over from
/// no particles first (a jump past `SCENE_CATCH_UP`), and the first and last tick to run.
/// None when nothing runs (paused, or a tick behind, which the caller starts over).
pub fn ticks_to_run(last: u64, tick: u64) -> Option<(bool, u64, u64)> {
    if tick <= last {
        return None;
    }
    let restart = tick - last > SCENE_CATCH_UP;
    let first = if restart {
        tick - SCENE_CATCH_UP + 1
    } else {
        last + 1
    };
    Some((restart, first, tick))
}

/// The camera turn that shows a particle's quad flat to the screen through
/// `scene_matrix`: its up up the screen and its right to the right.
pub fn facing_viewer(view: SceneView) -> Quat {
    Quat::from_rotation_y(-view.yaw.to_radians())
        * Quat::from_rotation_x(view.pitch.to_radians())
        * Quat::from_rotation_y(std::f32::consts::PI)
}

/// Whether `scene` lives: entities, particles to add, or blocks that animate. A scene
/// that doesn't takes the block-only path and keeps nothing.
pub fn lives(scene: &Scene) -> bool {
    !scene.entities.is_empty()
        || !scene.particles.is_empty()
        || scene.clock.is_some_and(|c| c.animate_blocks)
}

/// A living scene's structural input, checked before anything is budgeted, kept or drawn:
/// box, view (including positive finite `scale`), entity/particle caps, clock, and entity/particle
/// placements and angles. Entity handles/types/data and particle options are validated next.
pub fn check(rect: [f32; 4], scene: &Scene, view: SceneView) -> Result<SceneClock, SceneError> {
    let finite = rect.iter().chain(&view.center).all(|v| v.is_finite())
        && view.yaw.is_finite()
        && view.pitch.is_finite();
    if !finite || !valid_scale(view.scale) {
        return Err(SceneError::BadView);
    }
    if scene.entities.len() > SCENE_MAX_ENTITIES {
        return Err(SceneError::TooManyEntities);
    }
    if scene.particles.len() > SCENE_MAX_PARTICLES {
        return Err(SceneError::TooManyParticles);
    }
    let clock = scene.clock.ok_or(SceneError::NoClock)?;
    if !clock.partial.is_finite() {
        return Err(SceneError::BadClock);
    }
    let placed = |p: &[f64; 3]| p.iter().all(|v| v.is_finite() && v.abs() <= FAR);
    for (index, e) in (0u32..).zip(&scene.entities) {
        if !placed(&e.pos)
            || ![e.yaw, e.head_yaw, e.head_pitch]
                .iter()
                .all(|v| v.is_finite())
        {
            return Err(SceneError::BadEntity { index });
        }
    }
    for (index, p) in (0u32..).zip(&scene.particles) {
        if !placed(&p.pos) || !p.velocity.iter().all(|v| v.is_finite()) {
            return Err(SceneError::BadParticle { index });
        }
    }
    Ok(clock)
}

/// `DrawContext::entity`'s refusal for scene entity `index`, by the same name.
fn entity_refusal(error: GuiEntityError, index: u32) -> SceneError {
    match error {
        GuiEntityError::BadData => SceneError::BadData { index },
        GuiEntityError::NoEntity => SceneError::NoEntity { index },
        GuiEntityError::Unavailable | GuiEntityError::BadView => SceneError::Unavailable,
        _ => SceneError::UnknownType { index },
    }
}

impl crate::game::ClientGame<'_> {
    /// `DrawContext::draw_scene`: `blocks` (`scene.blocks`, prepared) into `rect` seen as
    /// `view` says. A scene of blocks alone takes the block-only path before any state
    /// work: nothing kept (if `scene.clock` has a finite partial, it still supplies block
    /// animation time). A living one is drawn with its entities and the particles its clock
    /// ticks. Refused whole, nothing drawn, budgeted or kept, for input that fails `check`,
    /// an unknown particle or an invalid `GuiEntity::Existing`; past the frame's animating
    /// budget the blocks and the scene's last state are drawn, its clock does not advance,
    /// and the call returns `TooManyScenes`; within the budget a fresh scene prepares its
    /// remaining entities once (reusing the already-prepared `Existing` actors) and refuses
    /// if one fails.
    pub fn native_draw_scene(
        &self,
        batch: &mut HudBatch,
        rect: [f32; 4],
        blocks: &[SceneBlock<'_>],
        scene: &Scene,
        view: SceneView,
    ) -> Result<(), SceneError> {
        if !self.common.native_window() {
            return Err(SceneError::Unavailable);
        }
        if !lives(scene) {
            let scene_clock = scene.clock.filter(|c| c.partial.is_finite()).map(|c| {
                let tick = c.tick as i64;
                (tick, c.partial, c.tick as f64 + f64::from(c.partial))
            });
            return self.draw_scene_quads(batch, rect, blocks, view, scene_clock, |_, _| {});
        }
        let owner = mod_api::content::crash::current_native_mod().unwrap_or("");
        let mut scenes = self
            .client
            .live_scenes
            .0
            .try_borrow_mut()
            .map_err(|_| SceneError::Unavailable)?;
        scenes.drive(
            self.client.frames,
            owner,
            rect,
            blocks,
            scene,
            view,
            |entity| self.prepare_gui_entity(entity),
            |clock, key, mobs| self.live_scene(blocks, scene, owner, clock, key, mobs),
            |l, clock, added| self.advance_live_scene(l, clock, added),
            |live, clock| match live {
                Some(l) => self.draw_live(batch, rect, blocks, view, l, clock.partial),
                None => {
                    let tick = clock.tick as i64;
                    let scene_clock = (
                        tick,
                        clock.partial,
                        clock.tick as f64 + f64::from(clock.partial),
                    );
                    self.draw_scene_quads(batch, rect, blocks, view, Some(scene_clock), |_, _| {})
                }
            },
        )
    }

    /// `l`'s blocks, entities and particles as they are, `partial` into the next tick.
    fn draw_live(
        &self,
        batch: &mut HudBatch,
        rect: [f32; 4],
        blocks: &[SceneBlock<'_>],
        view: SceneView,
        l: &mut Live,
        partial: f32,
    ) -> Result<(), SceneError> {
        let Live {
            tick,
            engine,
            mobs,
            verts,
            ..
        } = l;
        let scene_clock = (*tick as i64, partial, *tick as f64 + f64::from(partial));
        self.draw_scene_quads(
            batch,
            rect,
            blocks,
            view,
            Some(scene_clock),
            |quads, render_view| {
                for mob in mobs.iter() {
                    verts.clear();
                    // `DrawContext::entity`'s submission, in the scene's coordinates.
                    self.gui_entity_geometry_at(
                        mob,
                        partial,
                        DVec3::ZERO,
                        Some(render_view),
                        verts,
                    );
                    entity_quads(verts, quads);
                }
                verts.clear();
                let base = facing_viewer(view);
                engine.render(DVec3::ZERO, base, partial, &|_| 0xFF, verts);
                for q in verts.as_chunks::<4>().0 {
                    quads.push(SceneQuad {
                        corners: q.each_ref().map(|v| Vec3::from_array(v.pos)),
                        uv: q.each_ref().map(|v| v.uv),
                        color: q[0].color,
                        normal: Vec3::Z,
                        cull: false,
                    });
                }
            },
        )
    }

    /// A new kept scene: a level of `blocks`, an empty particle engine on the GUI atlas
    /// held to `SCENE_MAX_PARTICLES`, and `scene`'s already-prepared `mobs` (made for
    /// drawing only, nothing joins a level).
    fn live_scene(
        &self,
        blocks: &[SceneBlock<'_>],
        scene: &Scene,
        owner: &'static str,
        clock: SceneClock,
        key: u64,
        mobs: Vec<Mob>,
    ) -> Live {
        let mut world = World::new(self.common.res.facts().air, 0);
        let mut picks = Vec::new();
        let near = |v: i32| f64::from(v).abs() <= FAR;
        for b in blocks {
            if !(near(b.pos[0]) && near(b.pos[2])) {
                continue;
            }
            let at = (b.pos[0] >> 4, b.pos[2] >> 4);
            if world.chunks.get(&at).is_none() {
                world.chunks.insert(at, Arc::new(Chunk::empty(at.0, at.1)));
            }
            // Outside the level's height a block draws but has no animate tick.
            if world.set_stored(b.pos[0], b.pos[1], b.pos[2], b.state) {
                picks.push(b.pos);
            }
        }
        let mut engine = ParticleEngine::new(
            &self.common.pack.source,
            &self.client.gui.atlas.sprites,
            &self.client.gui.atlas.translucent,
        );
        engine.set_block_state_source(crate::game::particle_block_states);
        engine.live_limit = Some(SCENE_MAX_PARTICLES);
        // The spawn limiter's camera at the blocks' middle: the view never changes what lives.
        if !picks.is_empty() {
            let n = picks.len() as f64;
            let sum = picks.iter().fold([0.0; 3], |s, p| {
                [
                    s[0] + f64::from(p[0]),
                    s[1] + f64::from(p[1]),
                    s[2] + f64::from(p[2]),
                ]
            });
            engine.camera = sum.map(|v| v / n + 0.5);
        }
        Live {
            owner,
            id: clock.id,
            key,
            entities: scene.entities.iter().map(|e| e.entity.clone()).collect(),
            tick: clock.tick,
            frame: self.client.frames,
            world,
            picks,
            engine,
            mobs,
            verts: Vec::new(),
        }
    }

    /// Runs `l`'s ticks up to `clock.tick` as the client's tick runs them: the blocks'
    /// animate ticks (`SCENE_ANIMATE_PICKS` picks among the scene's own blocks), then the
    /// particles (`ParticleEngine.tick`). A jump past `SCENE_CATCH_UP` starts over from no
    /// particles and runs only the last `SCENE_CATCH_UP` ticks. The first tick run also
    /// adds `added`. Particles' sounds are dropped.
    fn advance_live_scene(
        &self,
        l: &mut Live,
        clock: SceneClock,
        added: &[(ParticleOptions, [f64; 3], [f64; 3])],
    ) {
        let Some((restart, first, last)) = ticks_to_run(l.tick, clock.tick) else {
            return;
        };
        let Live {
            world,
            engine,
            picks,
            ..
        } = l;
        if restart {
            engine.clear();
        }
        engine.particle_level = self.client.settings.particles;
        let facts = self.common.res.facts();
        let tables = &self.client.client_pack.context.mesh.tables;
        let model_offset = |state, x, z| tables.offset(state, x, z);
        let ctx = AnimateContext {
            enchant_power: &self.client.ambience.enchant_power,
            world,
            facts,
            collision: self.common.collision,
            biomes: world_biomes!(self),
            registry: self.common.pack.assets.registry,
            raining_at: &|_| false,
            height_no_leaves: &|_| i32::MIN,
            firefly_bush_sounds: false,
            creaking_active: false,
            sky_darken: 0,
            marker: None,
            ambient_particles: None,
            model_offset: &model_offset,
            gateway_particles: &|_, _| 0,
            thundering: false,
            ultrawarm: false,
            block_parts: None,
        };
        let level = ParticleLevel::new(world, facts, self.common.collision);
        for tick in first..=last {
            let seed = tick_seed(clock.id, tick);
            engine.seed_randoms(seed, seed.rotate_left(21), seed.rotate_left(42));
            engine.seed_particle_randoms(seed.rotate_left(53));
            if tick == first {
                for (options, at, velocity) in added {
                    engine.add_with(Some(&level), options, *at, *velocity, false, false);
                }
            }
            if clock.animate_blocks {
                animate_scene(
                    self.client_res().animate(),
                    engine,
                    &ctx,
                    picks,
                    SCENE_ANIMATE_PICKS,
                );
            }
            engine.tick(&level);
            engine.sounds.clear();
        }
        l.tick = clock.tick;
    }
}

/// `mob` placed and turned as `e` says, at the clock's tick (its age, which idle
/// animations read): no tick moves it.
fn pose(mob: &mut Mob, e: &mod_api::SceneEntity, clock: SceneClock) {
    let [x, y, z] = e.pos;
    mob.position = Position::new(x, y, z);
    mob.previous = mob.position;
    (mob.yaw, mob.previous_yaw) = (e.yaw, e.yaw);
    (mob.body_yaw, mob.previous_body_yaw) = (e.yaw, e.yaw);
    (mob.head_yaw, mob.previous_head_yaw) = (e.head_yaw, e.head_yaw);
    (mob.head_pitch, mob.previous_head_pitch) = (e.head_pitch, e.head_pitch);
    mob.age = clock.tick as u32;
}

/// An entity's quads as scene quads: shaded by face as the scene's blocks are, the
/// faces looking away dropped.
fn entity_quads(verts: &[Vertex], quads: &mut Vec<SceneQuad>) {
    for q in verts.as_chunks::<4>().0 {
        let corners = q.each_ref().map(|v| Vec3::from_array(v.pos));
        let normal = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .normalize_or_zero();
        quads.push(SceneQuad {
            corners,
            uv: q.each_ref().map(|v| v.uv),
            color: shaded(q[0].color, face_shade(normal)),
            normal,
            cull: normal != Vec3::ZERO,
        });
    }
}

#[cfg(any(test, feature = "test-support"))]
impl crate::game::ClientGame<'_> {
    /// The live particles of the mod's scene `id`: positions, previous positions and ages.
    pub fn scene_particles(&self, owner: &str, id: u32) -> Vec<([f64; 3], [f64; 3], i32)> {
        let scenes = self.client.live_scenes.0.borrow();
        scenes
            .live
            .iter()
            .find(|l| l.owner == owner && l.id == id)
            .map(|l| {
                l.engine
                    .live()
                    .map(|p| (p.position, p.previous, p.age))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// The tick the mod's scene `id` last ran, when it is kept.
    pub fn scene_tick(&self, owner: &str, id: u32) -> Option<u64> {
        let scenes = self.client.live_scenes.0.borrow();
        scenes
            .live
            .iter()
            .find(|l| l.owner == owner && l.id == id)
            .map(|l| l.tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::{EntityRef, SceneEntity, WorldEpoch};
    use std::collections::{HashMap, HashSet};
    use sunrise_assets::resource_source::ResourceSource;
    use sunrise_core::nbt::Tag;
    use sunrise_items::blocks::BlockFacts;
    use sunrise_level::particle_options::ParticleType;
    use sunrise_world::{
        DATA_VERSION,
        block_collision::BlockCollisionRegistry,
        collision::CachedCollisionRegistry,
        registry::{BlockRegistry, BlockStateId},
    };

    const RECT: [f32; 4] = [10.0, 10.0, 110.0, 90.0];
    const INVALID_SCALES: [f32; 6] = [0.0, -0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY];

    fn view_with_scale(scale: f32) -> SceneView {
        SceneView::new([0.5, 0.5, 0.5], 30.0, 25.0, scale)
    }

    fn good_view() -> SceneView {
        view_with_scale(20.0)
    }

    fn cow(pos: [f64; 3]) -> SceneEntity {
        SceneEntity::new(GuiEntity::kind("minecraft:cow"), pos)
    }

    fn mob_with_id(id: u32) -> Mob {
        Mob::new_remote(
            id,
            sunrise_sim::mobs::sensor_scan_tests::kind(),
            Position::ZERO,
            0.0,
        )
    }

    fn make_live(
        owner: &'static str,
        scene: &Scene,
        clock: SceneClock,
        key: u64,
        frame: u64,
        mobs: Vec<Mob>,
    ) -> Live {
        let mut engine = ParticleEngine::new(
            &ResourceSource::indexed(Default::default()),
            &HashMap::new(),
            &HashSet::new(),
        );
        engine.live_limit = Some(SCENE_MAX_PARTICLES);
        Live {
            owner,
            id: clock.id,
            key,
            entities: scene.entities.iter().map(|e| e.entity.clone()).collect(),
            tick: clock.tick,
            frame,
            world: World::new(BlockStateId(0), 0),
            picks: Vec::new(),
            engine,
            mobs,
            verts: Vec::new(),
        }
    }

    fn with_empty_particle_env(test: impl FnOnce(&BlockFacts, &BlockCollisionRegistry<'_>)) {
        let states = || Tag::List(vec![Tag::String("minecraft:air".into())]);
        let registry = BlockRegistry::from_nbt(&Tag::compound([
            ("DataVersion", Tag::Int(DATA_VERSION)),
            ("states", states()),
            ("defaults", Tag::compound([("minecraft:air", Tag::Int(0))])),
        ]))
        .unwrap();
        let cached = CachedCollisionRegistry::from_nbt(
            &Tag::compound([
                ("DataVersion", Tag::Int(DATA_VERSION)),
                ("states", states()),
                ("shapes", Tag::List(vec![])),
                ("cached_shapes", Tag::IntArray(vec![-1])),
            ]),
            &registry,
        )
        .unwrap();
        let collision = BlockCollisionRegistry::new(Arc::new(cached)).unwrap();
        let facts = BlockFacts {
            states: vec![],
            air: BlockStateId(0),
            woods: HashMap::new(),
            sign_texts: HashMap::new(),
        };
        test(&facts, &collision);
    }

    fn advance_with_particles(
        l: &mut Live,
        clock: SceneClock,
        facts: &BlockFacts,
        collision: &BlockCollisionRegistry<'_>,
    ) {
        let Some((restart, first, last)) = ticks_to_run(l.tick, clock.tick) else {
            return;
        };
        if restart {
            l.engine.clear();
        }
        let level = ParticleLevel::new(&l.world, facts, collision);
        let flame = ParticleOptions::simple(ParticleType::Flame);
        for tick in first..=last {
            let seed = tick_seed(clock.id, tick);
            l.engine
                .seed_randoms(seed, seed.rotate_left(21), seed.rotate_left(42));
            l.engine.seed_particle_randoms(seed.rotate_left(53));
            l.engine.add_with(
                Some(&level),
                &flame,
                [0.5, 1.5, 0.5],
                [0.0, 0.05, 0.0],
                false,
                false,
            );
            l.engine.tick(&level);
            l.engine.sounds.clear();
        }
        l.tick = clock.tick;
    }

    fn particle_snapshot(engine: &ParticleEngine) -> Vec<([u64; 3], [u64; 3], i32)> {
        engine
            .live()
            .map(|p| {
                (
                    p.position.map(f64::to_bits),
                    p.previous.map(f64::to_bits),
                    p.age,
                )
            })
            .collect()
    }

    #[test]
    fn invalid_scale_on_first_and_active_living_scenes_keeps_state_particles_and_budget_unchanged()
    {
        with_empty_particle_env(|facts, collision| {
            // 1) First call with invalid scale: refuses before preparing, keeping, or budgeting.
            let mut empty = LiveScenes::default();
            let first = Scene::new(Vec::new())
                .with_clock(SceneClock::new(1, 5, 0.25))
                .with_entities(vec![cow([0.5, 0.0, 0.5])]);
            for scale in INVALID_SCALES {
                let res = empty.drive(
                    10,
                    "guide",
                    RECT,
                    &[],
                    &first,
                    view_with_scale(scale),
                    |_| panic!("invalid scale must refuse before preparing entities"),
                    |_, _, _| panic!("invalid scale must refuse before making Live"),
                    |_, _, _| panic!("invalid scale must refuse before advancing"),
                    |_, _| panic!("invalid scale must refuse before drawing"),
                );
                assert_eq!(res, Err(SceneError::BadView), "scale {scale}");
                assert!(empty.is_empty());
                assert!(empty.animating.is_empty());
            }

            // 2) Already-active scene with live particles: invalid scale leaves frame, tick,
            // particles (exact bits), and next frame's animating budget untouched.
            let mut scenes = LiveScenes::default();
            let blocks = [SceneBlock {
                pos: [0, 0, 0],
                state: BlockStateId(1),
                entity: None,
            }];
            let base_scene = |tick| {
                Scene::new(Vec::new())
                    .with_clock(SceneClock::new(7, tick, 0.0))
                    .with_entities(vec![cow([0.5, 0.0, 0.5])])
            };
            for tick in [0, 5] {
                let s = base_scene(tick);
                assert_eq!(
                    scenes.drive(
                        10,
                        "guide",
                        RECT,
                        &blocks,
                        &s,
                        good_view(),
                        |_| Ok(mob_with_id(7)),
                        |clock, key, mobs| make_live("guide", &s, clock, key, 10, mobs),
                        |l, clock, _| advance_with_particles(l, clock, facts, collision),
                        |_, _| Ok(()),
                    ),
                    Ok(())
                );
            }

            let before_particles = particle_snapshot(&scenes.live[0].engine);
            assert!(!before_particles.is_empty());

            let next_scene = base_scene(8);
            for scale in [0.0, -1.0, f32::NAN] {
                let res = scenes.drive(
                    11,
                    "guide",
                    RECT,
                    &blocks,
                    &next_scene,
                    view_with_scale(scale),
                    |_| panic!("invalid scale must refuse before preparing entities"),
                    |_, _, _| panic!("invalid scale must refuse before replacing Live"),
                    |_, _, _| panic!("invalid scale must refuse before advancing"),
                    |_, _| panic!("invalid scale must refuse before drawing"),
                );
                assert_eq!(res, Err(SceneError::BadView), "scale {scale}");
                assert_eq!(scenes.live.len(), 1);
                assert_eq!(scenes.live[0].frame, 10);
                assert_eq!(scenes.live[0].tick, 5);
                assert_eq!(
                    particle_snapshot(&scenes.live[0].engine),
                    before_particles,
                    "particles unchanged for scale {scale}"
                );
                assert_eq!(scenes.frame, 10);
                assert_eq!(scenes.animating, vec![("guide", 7)]);
            }

            // Full frame-11 animating budget remains available for valid scenes.
            for id in 100..(100 + SCENE_MAX_ANIMATING as u32) {
                let other = Scene::new(Vec::new()).with_clock(SceneClock::new(id, 1, 0.0));
                assert_eq!(
                    scenes.drive(
                        11,
                        "guide",
                        RECT,
                        &blocks,
                        &other,
                        good_view(),
                        |_| Ok(mob_with_id(id)),
                        |clock, key, mobs| make_live("guide", &other, clock, key, 11, mobs),
                        |_, _, _| {},
                        |_, _| Ok(()),
                    ),
                    Ok(())
                );
            }
        });
    }

    #[test]
    fn fresh_living_scene_prepares_each_existing_actor_once_and_preserves_slot_order() {
        let mut scenes = LiveScenes::default();
        let ref_a = EntityRef::new(WorldEpoch::new(1), 10);
        let ref_b = EntityRef::new(WorldEpoch::new(1), 20);
        let entities = vec![
            SceneEntity::new(GuiEntity::kind("minecraft:cow"), [0.0, 0.0, 0.0]),
            SceneEntity::new(GuiEntity::existing(ref_a), [1.0, 0.0, 0.0]),
            SceneEntity::new(GuiEntity::kind("minecraft:pig"), [2.0, 0.0, 0.0]),
            SceneEntity::new(GuiEntity::existing(ref_b), [3.0, 0.0, 0.0]),
        ];
        let make_scene = |tick| {
            Scene::new(Vec::new())
                .with_clock(SceneClock::new(3, tick, 0.0).animate_blocks(false))
                .with_entities(entities.clone())
        };
        let mob_ids = |s: &LiveScenes| s.live[0].mobs.iter().map(|m| m.id).collect::<Vec<_>>();

        // 1) Fresh scene: Existing actors are prepared once up front, then remaining entities once
        // when building Live, landing in their original slot order.
        let mut prepared = Vec::new();
        let s0 = make_scene(10);
        let res = scenes.drive(
            1,
            "guide",
            RECT,
            &[],
            &s0,
            good_view(),
            |entity| {
                prepared.push(entity.clone());
                let id = match entity {
                    GuiEntity::Kind { kind, .. } if kind == "minecraft:cow" => 100,
                    GuiEntity::Kind { kind, .. } if kind == "minecraft:pig" => 102,
                    GuiEntity::Existing(r) if *r == ref_a => 201,
                    GuiEntity::Existing(r) if *r == ref_b => 203,
                    _ => return Err(GuiEntityError::UnknownType),
                };
                Ok(mob_with_id(id))
            },
            |clock, key, mobs| make_live("guide", &s0, clock, key, 1, mobs),
            |l, clock, _| l.tick = clock.tick,
            |_, _| Ok(()),
        );
        assert_eq!(res, Ok(()));
        assert_eq!(
            prepared,
            vec![
                GuiEntity::existing(ref_a),
                GuiEntity::existing(ref_b),
                GuiEntity::kind("minecraft:cow"),
                GuiEntity::kind("minecraft:pig"),
            ],
            "fresh scene must prepare each Existing actor once and each Kind once"
        );
        assert_eq!(mob_ids(&scenes), vec![100, 201, 102, 203]);

        // 2) Kept scene reuse: refreshes only Existing actors once each in place.
        prepared.clear();
        let s1 = make_scene(11);
        let res = scenes.drive(
            2,
            "guide",
            RECT,
            &[],
            &s1,
            good_view(),
            |entity| {
                prepared.push(entity.clone());
                let id = match entity {
                    GuiEntity::Existing(r) if *r == ref_a => 301,
                    GuiEntity::Existing(r) if *r == ref_b => 303,
                    _ => panic!("kept scene must not re-prepare non-Existing entity: {entity:?}"),
                };
                Ok(mob_with_id(id))
            },
            |_, _, _| panic!("kept scene must not be reconstructed"),
            |l, clock, _| l.tick = clock.tick,
            |_, _| Ok(()),
        );
        assert_eq!(res, Ok(()));
        assert_eq!(
            prepared,
            vec![GuiEntity::existing(ref_a), GuiEntity::existing(ref_b)]
        );
        assert_eq!(mob_ids(&scenes), vec![100, 301, 102, 303]);
    }

    #[test]
    fn budget_exhaustion_draws_fallback_and_preserves_existing_first_error_priority() {
        let ref_ok = EntityRef::new(WorldEpoch::new(1), 10);
        let ref_gone = EntityRef::new(WorldEpoch::new(1), u32::MAX);
        let arrow = SceneEntity::new(GuiEntity::kind("minecraft:arrow"), [0.0; 3]);
        let existing_ok = SceneEntity::new(GuiEntity::existing(ref_ok), [0.0; 3]);
        let existing_gone = SceneEntity::new(GuiEntity::existing(ref_gone), [0.0; 3]);

        let prepare = |entity: &GuiEntity| match entity {
            GuiEntity::Kind { kind, .. } if kind == "minecraft:cow" => Ok(mob_with_id(1)),
            GuiEntity::Kind { kind, .. } if kind == "minecraft:arrow" => {
                Err(GuiEntityError::UnknownType)
            }
            GuiEntity::Existing(r) if *r == ref_ok => Ok(mob_with_id(2)),
            GuiEntity::Existing(_) => Err(GuiEntityError::NoEntity),
            _ => Err(GuiEntityError::UnknownType),
        };

        // 1) Mixed [unknown Kind, missing Existing]: Existing is checked first, preserving
        // upstream's NoEntity { index: 1 } priority without spending budget or preparing Kind.
        let mut scenes = LiveScenes::default();
        let mixed = Scene::new(Vec::new())
            .with_clock(SceneClock::new(5, 1, 0.0).animate_blocks(false))
            .with_entities(vec![arrow.clone(), existing_gone.clone()]);
        let mut calls = Vec::new();
        let res = scenes.drive(
            10,
            "guide",
            RECT,
            &[],
            &mixed,
            good_view(),
            |e| {
                calls.push(e.clone());
                prepare(e)
            },
            |_, _, _| panic!("must not build Live"),
            |_, _, _| panic!("must not advance"),
            |_, _| panic!("must not draw"),
        );
        assert_eq!(res, Err(SceneError::NoEntity { index: 1 }));
        assert_eq!(calls, vec![GuiEntity::existing(ref_gone)]);
        assert!(scenes.is_empty());
        assert!(scenes.animating.is_empty());

        // 2) Fill the frame's animating budget (SCENE_MAX_ANIMATING scenes).
        for id in 0..SCENE_MAX_ANIMATING as u32 {
            let valid = Scene::new(Vec::new()).with_clock(SceneClock::new(id, 1, 0.0));
            assert_eq!(
                scenes.drive(
                    10,
                    "guide",
                    RECT,
                    &[],
                    &valid,
                    good_view(),
                    |_| Ok(mob_with_id(id)),
                    |clock, key, mobs| make_live("guide", &valid, clock, key, 10, mobs),
                    |_, _, _| {},
                    |_, _| Ok(()),
                ),
                Ok(())
            );
        }

        // 3) Past the budget: a new scene with Kind (valid or invalid) draws fallback blocks and
        // returns TooManyScenes without preparing Kind entities.
        for entity in [arrow.clone(), cow([0.0; 3])] {
            let over = Scene::new(Vec::new())
                .with_clock(SceneClock::new(99, 1, 0.0).animate_blocks(false))
                .with_entities(vec![entity]);
            let mut drew_fallback_blocks = false;
            let res = scenes.drive(
                10,
                "guide",
                RECT,
                &[],
                &over,
                good_view(),
                |e| panic!("over-budget Kind scene must not prepare entity: {e:?}"),
                |_, _, _| panic!("over-budget scene must not build Live"),
                |_, _, _| panic!("over-budget scene must not advance"),
                |live, _| {
                    assert!(live.is_none(), "new over-budget scene draws blocks only");
                    drew_fallback_blocks = true;
                    Ok(())
                },
            );
            assert_eq!(res, Err(SceneError::TooManyScenes));
            assert!(drew_fallback_blocks);
        }

        // 4) Past the budget with an invalid Existing handle: Existing validation still runs
        // before the fallback draw and returns NoEntity.
        let over_bad_existing = Scene::new(Vec::new())
            .with_clock(SceneClock::new(100, 1, 0.0).animate_blocks(false))
            .with_entities(vec![existing_gone]);
        let res = scenes.drive(
            10,
            "guide",
            RECT,
            &[],
            &over_bad_existing,
            good_view(),
            prepare,
            |_, _, _| panic!("must not build Live"),
            |_, _, _| panic!("must not advance"),
            |_, _| panic!("invalid Existing must refuse before fallback draw"),
        );
        assert_eq!(res, Err(SceneError::NoEntity { index: 0 }));

        // 5) Within budget on next frame, [valid Existing, unknown Kind] prepares each once and
        // returns UnknownType { index: 1 }.
        calls.clear();
        let bad_kind = Scene::new(Vec::new())
            .with_clock(SceneClock::new(101, 1, 0.0).animate_blocks(false))
            .with_entities(vec![existing_ok, arrow]);
        let res = scenes.drive(
            11,
            "guide",
            RECT,
            &[],
            &bad_kind,
            good_view(),
            |e| {
                calls.push(e.clone());
                prepare(e)
            },
            |_, _, _| panic!("must not build Live"),
            |_, _, _| panic!("must not advance"),
            |_, _| panic!("must not draw"),
        );
        assert_eq!(res, Err(SceneError::UnknownType { index: 1 }));
        assert_eq!(
            calls,
            vec![
                GuiEntity::existing(ref_ok),
                GuiEntity::kind("minecraft:arrow"),
            ]
        );
    }
}
