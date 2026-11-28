//! `Entity.isInvisibleTo`: a spectator sees an invisible body as the translucent
//! one (`LivingEntityRenderer.submit`'s `forceTransparent`), anyone else not at all.
#![cfg(feature = "client")]
use crate::game::{Audio, Settings, load_resources};
use std::sync::Arc;
use sunrise_command::args::GameMode;
use sunrise_core::geometry::Vec3;
use sunrise_level::chunk::Chunk;
use sunrise_protocol::{
    connection::Pipe,
    packets::play::{self, Clientbound as C},
};
use sunrise_sim::mobs::{Mob, MobTypes};
use {
    sunrise_gameplay_common::game::Options, sunrise_gameplay_common::game::Progress,
    sunrise_gameplay_common::game::execution_role,
};

/// The prepared vanilla resources.
fn root() -> std::path::PathBuf {
    crate::prepared_test_root::prepared_root(&[])
}

fn game<'a>(
    resources: &'a mut Option<std::sync::Arc<sunrise_gameplay_common::game::Resources>>,
) -> (sunrise_client_gameplay::game::ClientGame<'a>, Pipe) {
    let root = root();
    let settings = Settings::default();
    let loaded = load_resources(
        sunrise_assets::resource_source::ResourceRoot::directory(root.clone()),
        Vec::new(),
        None,
        &root,
        &settings,
        std::time::Instant::now(),
        false,
        sunrise_gameplay_common::game::execution_role::ExecutionRole::Client,
        &Progress::new(),
        &mut |_, _| unreachable!(),
    )
    .unwrap();
    let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
    options.resources = root.clone();
    options.execution_role = execution_role::ExecutionRole::Client;
    let res = &*resources.insert(loaded.resources);
    let mut game = sunrise_client_gameplay::game::ClientGame::new(
        res,
        loaded.client_res.unwrap(),
        loaded.pack,
        loaded.client_pack.unwrap(),
        loaded.gui.unwrap(),
        options,
        settings,
        Audio::silent(&sunrise_assets::resource_source::ResourceSource::directory(
            root.clone(),
        )),
        None,
        sunrise_gameplay::game::server_side::integrated::serve,
        sunrise_gameplay::game::server_side::integrated::run_component_harness_on_server,
        crate::game::session_verifier(),
        crate::game::profile_resolver,
    )
    .unwrap();
    let server =
        sunrise_client_gameplay::game::client_side::native_client_session::tests::logged_client(
            &mut game,
        );
    // The atlas the game bakes at startup: `mob_geometry` finds a body's texture in it.
    game.client.gui.atlas = sunrise_client::hud::HudAtlas::build(
        &sunrise_assets::resource_source::ResourceSource::directory(root.clone()),
    )
    .unwrap();
    game.world
        .chunks
        .insert((0, 0), Arc::new(Chunk::empty(0, 0)));
    let stone = game
        .pack
        .assets
        .registry
        .default_state("minecraft:stone")
        .unwrap();
    for x in 0..16 {
        for z in 0..16 {
            game.world.set(x, 63, z, stone).unwrap();
        }
    }
    (game, server)
}

/// `/effect give @e[type=minecraft:pig,limit=1] minecraft:invisibility 100 0`
/// sets the shared invisible flag a client reads (`LivingEntity.updateInvisibilityStatus`).
/// `Entity.isInvisibleTo` is false for a spectator, so `LivingEntityRenderer.submit`
/// draws the body it cannot show as visible as `entityTranslucentCull` at
/// `ARGB.multiply(654311423, getModelTint)`: alpha 38, in the blending pass.
#[test]
#[ignore = "daily"]
fn a_spectator_sees_the_invisible_body() {
    let mut resources = None;
    let (mut game, _server) = game(&mut resources);
    let profile = game.client.remote.as_ref().unwrap().profile();
    let types = MobTypes::load(&root()).unwrap();
    let kind = Arc::new(types.types["minecraft:pig"].clone());
    let mut pig = Mob::new(1, kind, Vec3::new(0.5, 64.0, 0.5), 0.0);
    pig.invisible = true;
    game.mobs.push(pig);
    let camera = glam::DVec3::new(6.5, 65.0, 6.5);

    // Anyone but a spectator sees no body at all.
    game.handle_packet(C::PlayerInfoUpdate(play::PlayerInfoUpdate {
        actions: 1 | 4,
        entries: vec![play::PlayerInfo {
            uuid: profile,
            name: Some("SpectatorViewTest".into()),
            game_mode: Some(GameMode::Survival.id()),
            ..Default::default()
        }],
    }))
    .unwrap();
    game.set_local_mode(GameMode::Survival);
    let mut out = Vec::new();
    game.mob_geometry(camera, 0.0, &mut out);
    assert!(out.is_empty(), "an invisible body is hidden from a player");

    // A spectator sees it forced translucent.
    game.handle_packet(C::PlayerInfoUpdate(play::PlayerInfoUpdate {
        actions: 4,
        entries: vec![play::PlayerInfo {
            uuid: profile,
            game_mode: Some(GameMode::Spectator.id()),
            ..Default::default()
        }],
    }))
    .unwrap();
    game.set_local_mode(GameMode::Spectator);
    let mut out = Vec::new();
    game.mob_geometry(camera, 0.0, &mut out);
    assert!(!out.is_empty(), "a spectator sees the invisible body");
    assert!(
        out.iter().all(|v| v.color >> 24 == 0x26),
        "the body is forced translucent (alpha 38)"
    );
    assert!(
        out.iter()
            .all(|v| v.layer & sunrise_client::gfx::OIT_ENTITY != 0),
        "the translucent body blends"
    );
}

/// Issue #6915: an invisible illusioner hides its real body and draws its four scattered mirror
/// images (plus held bow and worn head item at each mirror, using the vanilla 26.3 float-pose
/// translation order) through the real `begin_mob_geometry` / `mob_run_geometry` ->
/// `living_geometry` pass for an ordinary survival viewer (both `LivingFrame.skinning = false`
/// and `LivingFrame.skinning = true`, verified against a visible GPU-skinned control and exact
/// bit-for-bit placements including GPU-skinned bow matrices), keeps all four mirrors visible
/// alongside their outline when glowing, and stays opaque for a spectator. Runs in a fresh child
/// process so `load_resources` never mutates the shared test process's block namespace table.
#[test]
#[ignore = "daily"]
fn an_invisible_illusioner_draws_four_mirrors_for_an_ordinary_player() {
    const CHILD: &str = "SUNRISE_ILLUSIONER_MIRRORS_CHILD";
    if std::env::var_os(CHILD).is_none() {
        crate::fresh_process::run_in_fresh_process(
            "cut_layout::game::creatures::spectator_view_tests::an_invisible_illusioner_draws_four_mirrors_for_an_ordinary_player",
            CHILD,
            "1",
        );
        return;
    }

    use sunrise_items::items::ItemStack;
    use sunrise_sim::mobs::illusioner;

    let mut resources = None;
    let (mut game, _server) = game(&mut resources);
    let profile = game.client.remote.as_ref().unwrap().profile();
    let types = MobTypes::load(&root()).unwrap();
    let kind = Arc::new(types.types["minecraft:illusioner"].clone());
    // Non-dyadic camera-relative coordinates so folding the mirror offset in `f64` before
    // rounding to `f32` differs by a float step from vanilla's rounded origin + float translate.
    let mob_pos = Vec3::new(10.3, 64.0, 0.5);
    let camera = glam::DVec3::new(0.1, 65.0, 6.5);
    let alpha = 0.25_f64;

    let mut mob = Mob::new(1, kind, mob_pos, 0.0);
    mob.age = 13;
    // Vanilla `IllusionerRenderer`'s `ItemInHandLayer` shows the bow only when aggressive or casting.
    mob.ai.aggressive = true;

    let set_mode = |game: &mut sunrise_client_gameplay::game::ClientGame<'_>, mode: GameMode| {
        game.handle_packet(C::PlayerInfoUpdate(play::PlayerInfoUpdate {
            actions: 1 | 4,
            entries: vec![play::PlayerInfo {
                uuid: profile,
                name: Some("IllusionerMirrorTest".into()),
                game_mode: Some(mode.id()),
                ..Default::default()
            }],
        }))
        .unwrap();
        game.set_local_mode(mode);
    };
    set_mode(&mut game, GameMode::Survival);

    let draw_mobs_at =
        |game: &sunrise_client_gameplay::game::ClientGame<'_>, cam: glam::DVec3, skinning: bool| {
            game.client.skin_frame.borrow_mut().clear_draws();
            let mut frame = game.begin_mob_geometry(cam, alpha);
            frame.living.skinning = skinning;
            let mut out = Vec::new();
            game.mob_run_geometry(&frame, 0..game.mobs.len(), &mut out);
            let skin = game.client.skin_frame.borrow();
            let matrices: Vec<[f32; 16]> = skin
                .jobs
                .iter()
                .map(|j| skin.parts[j.part as usize].matrix)
                .collect();
            (out, matrices)
        };

    // Baseline: visible bare illusioner (both CPU and GPU-skinned positive control).
    let bare = mob.clone();
    game.mobs.push(bare.clone());
    let (visible_body_only, bare_cpu_mats) = draw_mobs_at(&game, camera, false);
    assert!(bare_cpu_mats.is_empty());
    let body_verts = visible_body_only.len();
    assert!(body_verts > 0, "visible illusioner draws a body");
    let (bare_skinned_out, bare_skin_mats) = draw_mobs_at(&game, camera, true);
    assert!(
        bare_skinned_out.is_empty(),
        "visible illusioner body should pose on GPU when LivingFrame.skinning = true"
    );
    assert!(
        !bare_skin_mats.is_empty(),
        "visible illusioner should emit GPU skin jobs when LivingFrame.skinning = true"
    );

    // Add both held bow and worn head item (`CustomHeadLayer`) so both equipment paths are checked.
    mob.extra
        .set_main_hand_stack(Some(ItemStack::new("minecraft:bow", 1)));
    mob.extra.set_slot_stack(
        sunrise_items::equipment::HEAD,
        Some(ItemStack::new("minecraft:carved_pumpkin", 1)),
    );
    game.mobs[0] = mob.clone();
    let (visible_with_gear, _) = draw_mobs_at(&game, camera, false);
    let gear_verts = visible_with_gear.len() - body_verts;
    assert!(
        gear_verts > 0,
        "visible illusioner draws held bow and head item"
    );
    let (vis_gear_skinned_out, vis_gear_skin_mats) = draw_mobs_at(&game, camera, true);
    let bow_skin_job_count = vis_gear_skin_mats.len() - bare_skin_mats.len();
    let gear_cpu_verts_when_skinned = vis_gear_skinned_out.len();
    assert!(
        bow_skin_job_count > 0,
        "held bow should take the GPU-skinned item path when LivingFrame.skinning = true"
    );

    // Scattered offsets + independent 26.3 Mth.cos wobble at age = 13.25.
    let offsets = [
        [2.5, 1.0, -1.5],
        [-2.0, 0.0, 2.0],
        [1.5, 1.0, 2.5],
        [-3.0, 0.0, -2.5],
    ];
    let want_wobble: [[f64; 3]; illusioner::COPIES] = [
        [
            f64::from_bits(0x3f98_1e83_cccc_cccd),
            f64::from_bits(0xbf86_4f29_4ccc_cccd),
            f64::from_bits(0xbf99_5028_0000_0000),
        ],
        [
            f64::from_bits(0x3f77_3fda_1999_999a),
            f64::from_bits(0xbf47_c8bf_e666_6667),
            f64::from_bits(0xbf90_e43c_b333_3333),
        ],
        [
            f64::from_bits(0xbf91_d6f9_9999_999a),
            f64::from_bits(0x3f84_b39b_e666_6667),
            f64::from_bits(0x3f7c_3cc8_3333_3334),
        ],
        [
            f64::from_bits(0xbf99_16b9_4ccc_cccd),
            f64::from_bits(0x3f87_db6c_b333_3334),
            f64::from_bits(0x3f98_8536_1999_999a),
        ],
    ];

    // Confirm this fixture genuinely separates vanilla's float-pose order from folding in `f64`.
    let origin_f32 = glam::DVec3::new(
        mob_pos.x - camera.x,
        mob_pos.y - camera.y,
        mob_pos.z - camera.z,
    )
    .as_vec3();
    let mirror_origins_f32: [glam::Vec3; illusioner::COPIES] = std::array::from_fn(|i| {
        let off = glam::Vec3::new(
            (offsets[i][0] + want_wobble[i][0]) as f32,
            (offsets[i][1] + want_wobble[i][1]) as f32,
            (offsets[i][2] + want_wobble[i][2]) as f32,
        );
        origin_f32 + off
    });
    assert!(
        (0..illusioner::COPIES).any(|i| {
            let folded = glam::DVec3::new(
                (mob_pos.x + offsets[i][0] + want_wobble[i][0]) - camera.x,
                (mob_pos.y + offsets[i][1] + want_wobble[i][1]) - camera.y,
                (mob_pos.z + offsets[i][2] + want_wobble[i][2]) - camera.z,
            )
            .as_vec3();
            folded.x.to_bits() != mirror_origins_f32[i].x.to_bits()
                || folded.y.to_bits() != mirror_origins_f32[i].y.to_bits()
                || folded.z.to_bits() != mirror_origins_f32[i].z.to_bits()
        }),
        "fixture must distinguish rounded-origin + f32 translate from f64 camera shift"
    );

    // Build bit-exact expected placements for each of the 4 mirrors by rendering a visible copy
    // at `camera = ZERO` with position equal to the exact vanilla-ordered `f32` origin.
    let mut want_body_pos = Vec::with_capacity(illusioner::COPIES);
    let mut want_bow_cpu_pos = Vec::with_capacity(illusioner::COPIES);
    let mut want_head_cpu_pos = Vec::with_capacity(illusioner::COPIES);
    let mut want_bow_skin_mats = Vec::with_capacity(illusioner::COPIES * bow_skin_job_count);
    for target in mirror_origins_f32 {
        let mut ref_mob = mob.clone();
        ref_mob.position = Vec3::new(
            f64::from(target.x),
            f64::from(target.y),
            f64::from(target.z),
        );
        ref_mob.previous = ref_mob.position;
        // Split bow-only vs bow+head vertices cleanly so we match exact layer submission order.
        let mut bow_only_mob = ref_mob.clone();
        bow_only_mob
            .extra
            .set_slot_stack(sunrise_items::equipment::HEAD, None);
        game.mobs[0] = bow_only_mob;
        let (ref_cpu_bow, _) = draw_mobs_at(&game, glam::DVec3::ZERO, false);
        let bow_v_count = ref_cpu_bow.len() - body_verts;
        assert!(bow_v_count > 0);
        want_body_pos.push(
            ref_cpu_bow[..body_verts]
                .iter()
                .map(|v| v.pos.map(f32::to_bits))
                .collect::<Vec<_>>(),
        );
        want_bow_cpu_pos.push(
            ref_cpu_bow[body_verts..]
                .iter()
                .map(|v| v.pos.map(f32::to_bits))
                .collect::<Vec<_>>(),
        );
        let (_, ref_skin_mats) = draw_mobs_at(&game, glam::DVec3::ZERO, true);
        want_bow_skin_mats.extend_from_slice(&ref_skin_mats[bare_skin_mats.len()..]);

        game.mobs[0] = ref_mob;
        let (ref_cpu_all, _) = draw_mobs_at(&game, glam::DVec3::ZERO, false);
        let head_v_count = ref_cpu_all.len() - body_verts - bow_v_count;
        assert!(head_v_count > 0, "worn carved pumpkin draws head vertices");
        want_head_cpu_pos.push(
            ref_cpu_all[body_verts + bow_v_count..]
                .iter()
                .map(|v| v.pos.map(f32::to_bits))
                .collect::<Vec<_>>(),
        );
    }
    let bow_verts = want_bow_cpu_pos[0].len();
    let head_verts = want_head_cpu_pos[0].len();

    // Invisible + scattered offsets: verify exact placements for both `skinning = false` and `true`.
    mob.invisible = true;
    mob.extra.illusions = Some(illusioner::Illusions {
        ticks: 0,
        offsets: [offsets, offsets],
    });
    game.mobs[0] = mob.clone();

    for skinning in [false, true] {
        let (out, skin_mats) = draw_mobs_at(&game, camera, skinning);
        // First 4 * body_verts are the four CPU mirror bodies at exact bit-for-bit positions.
        let mirror_bodies = &out[..body_verts * illusioner::COPIES];
        assert!(
            mirror_bodies.iter().all(|v| v.color >> 24 == 0xFF),
            "mirrors draw opaque"
        );
        for (i, chunk) in mirror_bodies.chunks_exact(body_verts).enumerate() {
            let got: Vec<[u32; 3]> = chunk.iter().map(|v| v.pos.map(f32::to_bits)).collect();
            assert_eq!(
                got, want_body_pos[i],
                "skinning={skinning}: mirror body {i} vertices must match vanilla float-pose order bit-for-bit"
            );
        }
        if !skinning {
            assert!(
                skin_mats.is_empty(),
                "no GPU skin jobs when skinning is off"
            );
            assert_eq!(
                out.len(),
                (body_verts + bow_verts + head_verts) * illusioner::COPIES,
                "CPU path draws 4 mirror bodies + 4 bows + 4 head items"
            );
            let bow_slice = &out
                [body_verts * illusioner::COPIES..(body_verts + bow_verts) * illusioner::COPIES];
            for (i, chunk) in bow_slice.chunks_exact(bow_verts).enumerate() {
                let got: Vec<[u32; 3]> = chunk.iter().map(|v| v.pos.map(f32::to_bits)).collect();
                assert_eq!(
                    got, want_bow_cpu_pos[i],
                    "CPU mirror bow {i} vertices must match vanilla float-pose order bit-for-bit"
                );
            }
            let head_slice = &out[(body_verts + bow_verts) * illusioner::COPIES..];
            for (i, chunk) in head_slice.chunks_exact(head_verts).enumerate() {
                let got: Vec<[u32; 3]> = chunk.iter().map(|v| v.pos.map(f32::to_bits)).collect();
                assert_eq!(
                    got, want_head_cpu_pos[i],
                    "CPU mirror head item {i} vertices must match vanilla float-pose order bit-for-bit"
                );
            }
        } else {
            assert_eq!(
                skin_mats.len(),
                bow_skin_job_count * illusioner::COPIES,
                "only the 4 mirror bows use GPU skinning; mirror bodies stay on CPU"
            );
            assert_eq!(
                out.len(),
                (body_verts + gear_cpu_verts_when_skinned) * illusioner::COPIES,
                "skinning=true draws 4 CPU mirror bodies plus 4 CPU head items"
            );
            // Verify every GPU-skinned mirror bow's 4x4 matrix bit-for-bit!
            let got_bits: Vec<[u32; 16]> = skin_mats.iter().map(|m| m.map(f32::to_bits)).collect();
            let want_bits: Vec<[u32; 16]> = want_bow_skin_mats
                .iter()
                .map(|m| m.map(f32::to_bits))
                .collect();
            assert_eq!(
                got_bits, want_bits,
                "GPU-skinned mirror bow matrices must match vanilla float-pose order bit-for-bit"
            );
            // Worn head item stays CPU and matches exact mirror placements.
            let head_slice = &out[body_verts * illusioner::COPIES..];
            for (i, chunk) in head_slice.chunks_exact(head_verts).enumerate() {
                let got: Vec<[u32; 3]> = chunk.iter().map(|v| v.pos.map(f32::to_bits)).collect();
                assert_eq!(
                    got, want_head_cpu_pos[i],
                    "skinning=true mirror head item {i} vertices must match bit-for-bit"
                );
            }
        }
    }

    // Glowing + invisible: keeps all 4 mirrors AND records outlines for all 4.
    game.client.glowing_vertices.borrow_mut().clear();
    game.mobs[0].glowing = true;
    game.mobs[0].extra.set_main_hand_stack(None);
    game.mobs[0]
        .extra
        .set_slot_stack(sunrise_items::equipment::HEAD, None);
    let (glow_out, _) = draw_mobs_at(&game, camera, false);
    assert_eq!(glow_out.len(), body_verts * illusioner::COPIES);
    assert_eq!(
        game.client.glowing_vertices.borrow().len(),
        body_verts * illusioner::COPIES
    );

    // Spectator view: illusioner mirrors stay opaque (0xFF), not forced translucent (0x26).
    game.mobs[0].glowing = false;
    set_mode(&mut game, GameMode::Spectator);
    let (spec_out, _) = draw_mobs_at(&game, camera, false);
    assert_eq!(spec_out.len(), body_verts * illusioner::COPIES);
    assert!(spec_out.iter().all(|v| v.color >> 24 == 0xFF));
}
