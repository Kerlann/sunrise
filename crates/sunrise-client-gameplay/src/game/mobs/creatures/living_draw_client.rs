use super::*;
use crate::game::{mob_blocks::MobView, stock_layer_mask::StockLayerMask};
use sunrise_client::mobs::render::FrameDraw;
use sunrise_gameplay_common::game::creatures::living_draw::LivingFrame;

/// Visibility and body-pass routing for one living entity (`LivingEntityRenderer.submit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LivingVisibility {
    invisible: bool,
    illusioner_mirrors: bool,
    seen_invisible: bool,
    layers_only: bool,
    /// Whether the glowing silhouette (and held-item GPU pass) may use GPU skinning.
    /// An invisible illusioner's mirror body is kept on the CPU separately via `!illusioner_mirrors`.
    silhouette_on_gpu: bool,
}

impl LivingVisibility {
    fn of(mob: &Mob, is_player: bool, outline: Option<u32>, seen_invisible: bool) -> Self {
        let invisible = mob.is_invisible();
        // `IllusionerRenderer.isBodyVisible` is always true: an invisible illusioner
        // hides its real body and draws its four mirror images in the normal render type.
        let illusioner_mirrors = invisible && mob.species() == Species::Illusioner;
        let layers_only = invisible && outline.is_none() && is_player && !seen_invisible;
        // Controls GPU outline eligibility (`SkinFrame::outline_since`) when glowing:
        // glowing invisible mobs take the CPU outline path instead.
        let silhouette_on_gpu = outline.is_none() || !invisible;
        Self {
            invisible,
            illusioner_mirrors,
            seen_invisible,
            layers_only,
            silhouette_on_gpu,
        }
    }

    fn skips(self, outline: Option<u32>) -> bool {
        self.invisible
            && outline.is_none()
            && !self.layers_only
            && !self.seen_invisible
            && !self.illusioner_mirrors
    }

    fn posable(
        self,
        mob: &Mob,
        skinning: bool,
        is_player: bool,
        modded: bool,
        armor_empty: bool,
    ) -> bool {
        skinning
            && !(is_player || modded)
            && !self.layers_only
            && !self.seen_invisible
            && !self.illusioner_mirrors
            && self.silhouette_on_gpu
            && armor_empty
            && !matches!(
                mob.species(),
                Species::Blaze | Species::MagmaCube | Species::GlowSquid | Species::SulfurCube
            )
    }

    /// Whether the glowing pass drew the body only to capture its silhouette and
    /// must drop the regular body vertices afterwards.
    fn outline_only(self) -> bool {
        self.invisible && !self.seen_invisible && !self.illusioner_mirrors
    }
}

impl crate::game::ClientGame<'_> {
    /// Draws `mob` into `out` as the frame's mob pass does, or nothing when it is out of range,
    /// out of view, hidden or culled.
    pub(in crate::game) fn living_geometry<const PRECHECK: bool>(
        &self,
        mob: &Mob,
        frame: &LivingFrame<'_>,
        out: &mut Vec<Vertex>,
    ) {
        let LivingFrame {
            camera,
            alpha,
            scale,
            viewer,
            skinning,
            listened,
            now,
        } = *frame;
        let atlas = &self.client.gui.atlas;
        let lookup = |name: &str| atlas.sprites.get(name).map(|r| [r.u0, r.v0, r.u1, r.v1]);
        // LevelRenderer: the entity the view looks out of is not drawn
        // in first person.
        if self.hides_as_camera(mob.id) {
            return;
        }
        if !self.entity_section_visible(
            glam::DVec3::new(mob.position.x, mob.position.y, mob.position.z),
            now,
        ) {
            return;
        }
        let is_player = &*mob.kind.id == "minecraft:player";
        let d = glam::DVec3::new(mob.position.x, mob.position.y, mob.position.z) - camera;
        // Entity.shouldRenderAtSqrDistance: 64 times the bounding box's
        // mean edge, scaled by the view scale. RemotePlayer's box counts
        // ten times.
        let (w, h, _) = mob.dims();
        let edge = (w + h + w) / 3.0 * if is_player { 10.0 } else { 1.0 };
        let range = edge * 64.0 * scale;
        if d.length_squared() >= range * range {
            return;
        }
        let beam = sunrise_sim::mobs::guardian::beam(mob, alpha as f32);
        // EntityRenderer.shouldRender: the renderer's interpolated culling box,
        // grown by 0.5 (GuardianRenderer also draws while its beam is out).
        if let Some(view) = &self.client.view_frustum
            && beam.is_none()
        {
            let (lo, hi) = super::super::entity_culling::cull_box(mob, alpha);
            let (lo, hi) = (
                (lo - camera - glam::DVec3::splat(0.5)).as_vec3(),
                (hi - camera + glam::DVec3::splat(0.5)).as_vec3(),
            );
            if !view.intersects(lo, hi) {
                return;
            }
        }
        let outline = self.mob_outline(mob);
        // Entity.DATA_SHARED_FLAGS_ID invisible (the flag the invisibility
        // effect sets): only the name tag draws (and the outline of a
        // glowing mob, LivingEntityRenderer.getRenderType). A player keeps
        // its armour, elytra and held items (`LivingEntityRenderer.submit`
        // skips only the body); an illusioner draws its four mirror images
        // (`IllusionerRenderer.isBodyVisible` is always true).
        let invisible = mob.is_invisible();
        // Entity.isInvisibleTo(viewer): a spectator sees it anyway, the
        // body forced translucent (`LivingEntityRenderer.submit`).
        let seen_invisible = invisible
            && !sunrise_game_data::scoreboard::invisible_to(
                self.client_mob_team_of(mob),
                viewer,
                self.own_player_is_spectator(),
                true,
            );
        let vis = LivingVisibility::of(mob, is_player, outline, seen_invisible);
        if vis.skips(outline) {
            return;
        }
        let culling = if PRECHECK
            && outline.is_none()
            && !is_player
            && mob.kind.id.starts_with("minecraft:")
            && (0..6).all(|slot| mob.extra.slot_stack(slot).is_none())
            && beam.is_none()
        {
            let Some(bounds) = super::super::entity_client::mob_render_bounds(
                mob,
                self.client_res().client_mob_models(),
                alpha,
            ) else {
                return;
            };
            self.entity_cull_decision_in_bounds(mob, bounds)
        } else {
            EntityCullDecision::CheckVertices
        };
        if culling.hides_entity() {
            return;
        }
        let entity_start = out.len();
        self.mark_motion(entity_start, (1 << 32) | u64::from(mob.id));
        self.mark_entity(entity_start, &mob.kind.id);
        self.client
            .rendered_mobs
            .set(self.client.rendered_mobs.get() + 1);
        let packed = self.mob_packed_light(mob, alpha);
        // The mods' pre hooks (a player's `render_player_pre`, then `render_living_pre`),
        // before any of the vanilla model's parts (a sulfur cube's swallowed block) are
        // drawn. A cancel skips the rest of the entity's drawing and its post hooks; the
        // frame's later name tag and leash passes leave it out too, its flames and shadow stay.
        let who = is_player
            .then(|| self.remote_player_name(mob.id))
            .flatten()
            .map(|name| PlayerRender { name, local: false });
        let mut frame_draw = FrameDraw::default();
        if !self.mod_render_pre(
            mob,
            who,
            listened,
            alpha as f32,
            camera,
            packed,
            &mut frame_draw,
            out,
        ) {
            // What the mods drew in its place is culled as the entity's own would be.
            self.cull_entity_vertices(out, entity_start);
            return;
        }
        // A mod's renderer for the type (`register_entity_renderers`) draws in place of its
        // model and vanilla layers; the mods' layers and post hooks still follow.
        let modded =
            self.mod_living_renderer_geometry(mob, alpha as f32, camera, packed, true, out);
        // AvatarRenderer.shouldRenderLayers: a spectator draws no layers.
        let layers = sunrise_client::mobs::render::layers_shown(mob);
        let resolved = self.entity_layers(mob);
        let hidden = if modded {
            StockLayerMask::EMPTY
        } else {
            resolved.hidden
        };
        if mob.species() == Species::SulfurCube && !mob.is_invisible() && !modded {
            self.sulfur_cube_body_geometry(
                mob,
                alpha as f32,
                sunrise_client::mobs::render::base_transform(mob, alpha as f32, camera),
                packed,
                out,
            );
        }
        // Other players on a server: PlayerRenderer's HumanoidArmorLayer.
        let armor = if is_player {
            self.worn_armor_layers(mob, hidden)
        } else {
            Vec::new()
        };
        let body = out.len();
        // The GPU path: an ordinary mob, no layers the CPU path changes after drawing them
        // (a player's, the HDR tiers below, the sulfur cube's shell, an invisible illusioner's
        // four mirrors).
        let posable = vis.posable(mob, skinning, is_player, modded, armor.is_empty());
        let added = if layers { resolved.layers } else { None };
        let data = super::super::humanoid_setup_anim::HiddenLayersData {
            game: self,
            hidden_layers: hidden,
        };
        frame_draw.capture_model = !modded
            && (added.is_some() || (listened && self.mob_render_post_listened(who.is_some())));
        let mut skinned = None;
        if posable {
            let mut models = self.client.skin_models.0.borrow_mut();
            let mut frame = self.client.skin_frame.borrow_mut();
            let mark = frame.mark();
            let mut skin = sunrise_client::mobs::skin::Skin::new(&mut models, &mut frame);
            sunrise_client::mobs::render::render_skinned(
                mob,
                alpha as f32,
                camera,
                &lookup,
                packed,
                &data,
                &mut frame_draw,
                out,
                &mut skin,
            );
            skinned = Some((mark, skin.bounds));
        } else if !modded {
            if vis.seen_invisible {
                sunrise_client::mobs::render::render_seen_invisible(
                    mob,
                    alpha as f32,
                    camera,
                    &lookup,
                    packed,
                    &data,
                    &armor,
                    &mut frame_draw,
                    out,
                );
            } else if vis.layers_only {
                sunrise_client::mobs::render::render_layers(
                    mob,
                    alpha as f32,
                    camera,
                    &lookup,
                    packed,
                    &data,
                    &armor,
                    &mut frame_draw,
                    out,
                );
            } else {
                sunrise_client::mobs::render::render_with_frame(
                    mob,
                    alpha as f32,
                    camera,
                    &lookup,
                    packed,
                    &data,
                    &armor,
                    &mut frame_draw,
                    out,
                );
            }
        }
        let model = frame_draw.model();
        // HDR output: mobs that are light shine by their kind, as blocks do (entity.wgsl
        // fs_hdr_mark); a tier's window lifts only their bright texels.
        if let Some(tier) = match mob.species() {
            Species::Blaze => Some(sunrise_client::mesher::BRIGHT),
            Species::MagmaCube | Species::GlowSquid => Some(sunrise_client::mesher::LOW),
            _ => None,
        } {
            for v in &mut out[body..] {
                v.layer |= u32::from(tier) << 16;
            }
        }
        if is_player && layers {
            // PlayerRenderer's CapeLayer, WingsLayer, ParrotOnShoulderLayer
            // and SpinAttackEffectLayer. Only the cape hides with the body;
            // the others have no `isInvisible` check.
            let chest = mob.extra.slot_id(sunrise_items::equipment::CHEST);
            // `CapeLayer` and `Deadmau5EarsLayer`: `!state.isInvisible`, whoever sees the body.
            if !invisible && !hidden.contains(mod_api::StockLayer::Cape) {
                self.player_cape_geometry(mob, chest, camera, alpha as f32, packed, out);
            }
            if !invisible
                && !hidden.contains(mod_api::StockLayer::Deadmau5Ears)
                && let Some(who) = who
            {
                self.player_ears_geometry(mob, who.name, camera, alpha as f32, packed, out);
            }
            if !hidden.contains(mod_api::StockLayer::Wings) {
                self.player_elytra_geometry(mob, chest, camera, alpha as f32, packed, out);
            }
            if !hidden.contains(mod_api::StockLayer::ParrotOnShoulder) {
                self.shoulder_parrots_geometry(mob, camera, alpha as f32, packed, out);
            }
            if !hidden.contains(mod_api::StockLayer::SpinAttackEffect) {
                self.spin_attack_geometry(mob, camera, alpha as f32, packed, out);
            }
            let view = MobView {
                camera,
                alpha,
                light: packed,
            };
            self.stuck_in_body_geometry(mob, mob.id as i32, view, hidden, out);
        }
        // CustomHeadLayer (a pumpkin, skull or head item) included.
        if layers {
            if !modded {
                // Held items join the GPU path's draws, their silhouette too.
                let mut skin = (skinning && vis.silhouette_on_gpu).then_some(&mut skinned);
                if vis.illusioner_mirrors {
                    for off in sunrise_sim::mobs::illusioner::mirror_offsets(mob, alpha as f32) {
                        let view = MobView {
                            camera: camera - glam::DVec3::from_array(off),
                            alpha,
                            light: packed,
                        };
                        self.mob_block_geometry(mob, view, hidden, skin.as_deref_mut(), out);
                    }
                } else {
                    let view = MobView {
                        camera,
                        alpha,
                        light: packed,
                    };
                    self.mob_block_geometry(mob, view, hidden, skin, out);
                }
            }
            let stock_ctx = super::super::native_entity_layers::StockLayerContext {
                is_player,
                local_player: false,
                invisible,
                in_world: true,
                player_name: who.map(|w| w.name),
                alpha,
                camera,
                light: packed,
                supported: if modded {
                    StockLayerMask::EMPTY
                } else {
                    resolved.supported
                },
                hidden,
            };
            self.mod_layer_geometry(
                mob,
                who,
                model,
                added,
                &stock_ctx,
                alpha as f32,
                camera,
                packed,
                out,
            );
        }
        self.mod_render_post(mob, who, model, alpha as f32, camera, packed, out);
        if let Some(rgb) = outline {
            self.mark_outline(&out[entity_start..], rgb);
            if let Some(((first, _, _), _)) = skinned {
                let color = sunrise_client::mobs::render::abgr(rgb);
                self.client
                    .skin_frame
                    .borrow_mut()
                    .outline_since(first, color);
            }
            if vis.outline_only() {
                out.truncate(entity_start);
                return;
            }
        }
        // Glowing outlines and a guardian's beam remain visible independently
        // of whether the body is hidden behind terrain.
        if outline.is_none() && culling.checks_vertices() && beam.is_none() {
            // What the GPU draws of the mob counts by its bounds' corners.
            self.cull_skinned(out, entity_start, skinned);
        }
        // GuardianRenderer: the laser to the locked target.
        if let Some((time, scale)) = beam
            && !modded
            && let Some(target) = self.beam_target(mob.ai.target, alpha)
            && let Some(uv) = lookup("entity/guardian/guardian_beam")
        {
            let a = alpha;
            let eye = glam::DVec3::new(
                mob.previous.x + (mob.position.x - mob.previous.x) * a,
                mob.previous.y + (mob.position.y - mob.previous.y) * a + mob.eye_height(),
                mob.previous.z + (mob.position.z - mob.previous.z) * a,
            ) - camera;
            sunrise_client::mobs_render::guardian_beam_geometry(
                eye.as_vec3(),
                (target - camera).as_vec3(),
                time,
                scale,
                uv,
                out,
            );
        }
    }
}

use sunrise_gameplay_common::game::game_mode_common::GameModeCommonGameExt1 as _;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use sunrise_client::{
        mobs::Part,
        mobs_render::{ClientMobData, ClientMobModels, ClientMobRender, ClientMobVisuals},
    };
    use sunrise_sim::mobs::{Anim, Brain, Flags, MobType, illusioner};

    fn synthetic_mob(id: u32, kind: &str, species: Species, anim: Anim) -> Mob {
        let mob_type = Arc::new(MobType {
            type_key: Default::default(),
            id: kind.into(),
            species,
            width: 0.6,
            height: 1.95,
            eye_height: 1.62,
            category: "monster".into(),
            max_health: 32.0,
            speed: 0.5,
            flying_speed: 0.0,
            attack_damage: 0.0,
            attack_knockback: 0.0,
            follow_range: 18.0,
            step_height: 0.6,
            armor: 0.0,
            armor_toughness: 0.0,
            knockback_resistance: 0.0,
            jump_strength: 0.42,
            attributes: Arc::default(),
            item_stack_limits: Arc::default(),
            attribute_bounds: Default::default(),
            gravity: 0.08,
            safe_fall_distance: 3.0,
            fall_damage_multiplier: 1.0,
            water_efficiency: 0.0,
            water_slowdown: 0.8,
            air_speed: 0.02,
            baby_scale: 0.5,
            head_rot_speed: 10.0,
            max_head_x_rot: 40.0,
            max_head_y_rot: 75.0,
            ambient_interval: 120,
            flags: Flags::default(),
            brain: Brain::Passive,
            anim,
            sounds: [""; 3],
            sound_overrides: Default::default(),
            food: Default::default(),
            tempt: Default::default(),
            passenger_attachments: Vec::new(),
        });
        let mut mob = Mob::new(id, mob_type, Vec3::new(4.0, 64.0, -2.0), 0.0);
        mob.random = sunrise_core::random::LegacyRandom::new(7);
        mob
    }

    fn synthetic_models(kind: &str, half_width_px: f32) -> ClientMobModels {
        let empty = |name: &str, pos: [f32; 3], children: Vec<Part>| Part {
            name: name.into(),
            pose: [pos[0], pos[1], pos[2], 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            polygons: Vec::new(),
            children,
            layer: None,
        };
        let body = Part {
            name: "body".into(),
            pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            polygons: vec![sunrise_client::mobs::Polygon {
                normal: [0.0, 0.0, 1.0],
                vertices: [
                    [-half_width_px, 0.0, 0.0, 0.0, 0.0],
                    [half_width_px, 0.0, 0.0, 1.0, 0.0],
                    [half_width_px, 12.0, 0.0, 1.0, 1.0],
                    [-half_width_px, 12.0, 0.0, 0.0, 1.0],
                ],
                cull_back: false,
            }],
            children: Vec::new(),
            layer: None,
        };
        let root = empty(
            "root",
            [0.0, 0.0, 0.0],
            vec![
                empty(
                    "head",
                    [0.0, 0.0, 0.0],
                    vec![empty("hat", [0.0; 3], vec![])],
                ),
                body,
                empty("arms", [0.0, 2.0, 0.0], vec![]),
                empty("right_arm", [-5.0, 2.0, 0.0], vec![]),
                empty("left_arm", [5.0, 2.0, 0.0], vec![]),
                empty("right_leg", [-2.0, 12.0, 0.0], vec![]),
                empty("left_leg", [2.0, 12.0, 0.0], vec![]),
            ],
        );
        let mut models = ClientMobModels::default();
        models.insert(
            kind.to_owned(),
            ClientMobVisuals {
                shadow: None,
                model: Arc::new(root),
                layers: Default::default(),
                texture: "entity/illager/illusioner".into(),
            },
        );
        models
    }

    #[test]
    fn living_visibility_routes_invisible_illusioners_without_affecting_ordinary_mobs() {
        let mut ill = synthetic_mob(
            1,
            "minecraft:illusioner",
            Species::Illusioner,
            Anim::Illager,
        );
        let mut zombie = synthetic_mob(2, "minecraft:zombie", Species::Zombie, Anim::Zombie);
        let mut player = synthetic_mob(
            3,
            "minecraft:player",
            Species::Zombie,
            Anim::Humanoid { zombie_arms: false },
        );

        // Visible illusioner can take GPU skinning.
        let vis = LivingVisibility::of(&ill, false, None, false);
        assert!(!vis.skips(None));
        assert!(vis.posable(&ill, true, false, false, true));

        // Invisible illusioner for ordinary viewer: stays in pass, falls back to CPU mirrors,
        // and keeps its mirrors when glowing.
        ill.invisible = true;
        let vis = LivingVisibility::of(&ill, false, None, false);
        assert!(!vis.skips(None));
        assert!(!vis.posable(&ill, true, false, false, true));
        let glow = LivingVisibility::of(&ill, false, Some(0x00FF_FFFF), false);
        assert!(!glow.skips(Some(0x00FF_FFFF)));
        assert!(!glow.outline_only());

        // Ordinary invisible mob still skips when not glowing and drops body when glowing.
        zombie.invisible = true;
        let z_vis = LivingVisibility::of(&zombie, false, None, false);
        assert!(z_vis.skips(None));
        let z_glow = LivingVisibility::of(&zombie, false, Some(0x00FF_FFFF), false);
        assert!(!z_glow.skips(Some(0x00FF_FFFF)));
        assert!(z_glow.outline_only());

        // Invisible player keeps layers_only.
        player.invisible = true;
        let p_vis = LivingVisibility::of(&player, true, None, false);
        assert!(p_vis.layers_only);
        assert!(!p_vis.skips(None));
    }

    #[test]
    fn invisible_illusioner_precheck_bounds_contain_max_offset_and_min_scale_mirrors() {
        let models = synthetic_models("minecraft:illusioner", 10.0);
        let max_offsets = [
            [3.0, 1.0, 3.0],
            [-3.0, 0.0, -3.0],
            [3.0, 1.0, -3.0],
            [-3.0, 0.0, 3.0],
        ];
        let lookup = |_: &str| Some([0.0, 0.0, 1.0, 1.0]);
        let data = ClientMobData::default();
        let render_data = ClientMobRender {
            models: &models,
            data: &data,
        };
        let camera = glam::DVec3::new(0.0, 64.0, 6.0);

        for scale in [1.0_f32, 0.0625_f32] {
            let mut mob = synthetic_mob(
                6,
                "minecraft:illusioner",
                Species::Illusioner,
                Anim::Illager,
            );
            mob.invisible = true;
            mob.age = 0;
            mob.extra.model_scale = scale;
            mob.extra.illusions = Some(illusioner::Illusions {
                ticks: 0,
                offsets: [max_offsets, max_offsets],
            });
            let mut out = Vec::new();
            let mut frame_draw = FrameDraw::default();
            sunrise_client::mobs::render::render_with_frame(
                &mob,
                0.0,
                camera,
                &lookup,
                0x00F0_00F0,
                &render_data,
                &[],
                &mut frame_draw,
                &mut out,
            );
            assert!(!out.is_empty());
            let bounds = crate::game::entity_client::mob_render_bounds(&mob, &models, 0.0).unwrap();
            let lo = bounds.min_position();
            let hi = bounds.max_position();
            let mut max_dx = 0.0_f64;
            let mut max_dy = 0.0_f64;
            for vertex in &out {
                let world = [
                    f64::from(vertex.pos[0]) + camera.x,
                    f64::from(vertex.pos[1]) + camera.y,
                    f64::from(vertex.pos[2]) + camera.z,
                ];
                max_dx = max_dx.max((world[0] - mob.position.x).abs());
                max_dy = max_dy.max(world[1] - mob.position.y);
                assert!(
                    world[0] >= lo.x - 1e-4
                        && world[0] <= hi.x + 1e-4
                        && world[1] >= lo.y - 1e-4
                        && world[1] <= hi.y + 1e-4
                        && world[2] >= lo.z - 1e-4
                        && world[2] <= hi.z + 1e-4,
                    "scale={scale}: mirror vertex {world:?} outside precheck bounds {lo:?}..{hi:?}"
                );
            }
            if scale == 1.0 {
                assert!(max_dx > 3.5, "got max_dx={max_dx}");
            } else {
                let (_, height, _) = mob.awake_dims();
                assert!(
                    max_dy > height * 3.0 + 0.5,
                    "min-scale fixture must exceed unexpanded vertical precheck bound (max_dy={max_dy}, old={})",
                    height * 3.0 + 0.5
                );
            }
        }
    }
}
