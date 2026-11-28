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
/// images (plus held bow at each mirror) through the real `begin_mob_geometry` / `mob_run_geometry`
/// -> `living_geometry` pass for an ordinary survival viewer (both `LivingFrame.skinning = false`
/// and `LivingFrame.skinning = true`, verified against a visible GPU-skinned control), keeps all
/// four mirrors visible alongside their outline when glowing, and stays opaque for a spectator.
#[test]
#[ignore = "daily"]
fn an_invisible_illusioner_draws_four_mirrors_for_an_ordinary_player() {
    use sunrise_items::items::ItemStack;
    use sunrise_sim::mobs::illusioner;

    let mut resources = None;
    let (mut game, _server) = game(&mut resources);
    let profile = game.client.remote.as_ref().unwrap().profile();
    let types = MobTypes::load(&root()).unwrap();
    let kind = Arc::new(types.types["minecraft:illusioner"].clone());
    let mut mob = Mob::new(1, kind, Vec3::new(0.5, 64.0, 0.5), 0.0);
    mob.age = 13;
    // Vanilla `IllusionerRenderer`'s `ItemInHandLayer` shows the bow only when aggressive or casting.
    mob.ai.aggressive = true;
    let camera = glam::DVec3::new(0.5, 65.0, 6.5);
    let alpha = 0.25_f64;

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

    // Baseline: visible illusioner holding a bow draws 1 body + 1 held bow.
    let draw_mobs = |game: &sunrise_client_gameplay::game::ClientGame<'_>, skinning: bool| {
        game.client.skin_frame.borrow_mut().clear_draws();
        let mut frame = game.begin_mob_geometry(camera, alpha);
        frame.living.skinning = skinning;
        let mut out = Vec::new();
        game.mob_run_geometry(&frame, 0..game.mobs.len(), &mut out);
        let skin_jobs = game.client.skin_frame.borrow().jobs.len();
        (out, skin_jobs)
    };

    mob.extra
        .set_main_hand_stack(Some(ItemStack::new("minecraft:bow", 1)));
    let mut bare = mob.clone();
    bare.extra.set_main_hand_stack(None);
    game.mobs.push(bare);
    let (visible_body_only, bare_cpu_jobs) = draw_mobs(&game, false);
    assert_eq!(bare_cpu_jobs, 0);
    let body_verts = visible_body_only.len();
    assert!(body_verts > 0, "visible illusioner draws a body");
    let body_center = {
        let n = body_verts as f64;
        let s = visible_body_only.iter().fold([0.0; 3], |a, v| {
            [
                a[0] + f64::from(v.pos[0]),
                a[1] + f64::from(v.pos[1]),
                a[2] + f64::from(v.pos[2]),
            ]
        });
        [s[0] / n, s[1] / n, s[2] / n]
    };
    // Positive control: `LivingFrame.skinning = true` actually poses a visible illusioner body on GPU.
    let (bare_skinned_out, bare_skin_jobs) = draw_mobs(&game, true);
    assert!(
        bare_skinned_out.is_empty(),
        "visible illusioner body should pose on GPU when LivingFrame.skinning = true"
    );
    assert!(
        bare_skin_jobs > 0,
        "visible illusioner should emit GPU skin jobs when LivingFrame.skinning = true"
    );

    game.mobs.clear();
    game.mobs.push(mob.clone());
    let (visible_with_bow, _) = draw_mobs(&game, false);
    let bow_verts = visible_with_bow.len() - body_verts;
    assert!(bow_verts > 0, "visible illusioner draws its held bow");
    let (vis_bow_skinned_out, vis_bow_skin_jobs) = draw_mobs(&game, true);
    let bow_skin_jobs = vis_bow_skin_jobs - bare_skin_jobs;
    let bow_cpu_verts_when_skinned = vis_bow_skinned_out.len();
    assert!(
        bow_skin_jobs > 0 || bow_cpu_verts_when_skinned == bow_verts,
        "visible held bow accounts for either GPU skin jobs or CPU vertices"
    );
    let bow_center = {
        let slice = &visible_with_bow[body_verts..];
        let n = bow_verts as f64;
        let s = slice.iter().fold([0.0; 3], |a, v| {
            [
                a[0] + f64::from(v.pos[0]),
                a[1] + f64::from(v.pos[1]),
                a[2] + f64::from(v.pos[2]),
            ]
        });
        [s[0] / n, s[1] / n, s[2] / n]
    };

    // Invisible + scattered offsets: 4 opaque mirrors + 4 held bows for an ordinary viewer.
    let offsets = [
        [2.5, 1.0, -1.5],
        [-2.0, 0.0, 2.0],
        [1.5, 1.0, 2.5],
        [-3.0, 0.0, -2.5],
    ];
    // Independent 26.3 Mth.cos wobble at age = 13.25.
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
    mob.invisible = true;
    mob.extra.illusions = Some(illusioner::Illusions {
        ticks: 0,
        offsets: [offsets, offsets],
    });
    game.mobs[0] = mob.clone();

    for skinning in [false, true] {
        let (out, skinned_jobs) = draw_mobs(&game, skinning);
        if !skinning {
            assert_eq!(skinned_jobs, 0, "no GPU skin jobs when skinning is off");
            assert_eq!(
                out.len(),
                (body_verts + bow_verts) * illusioner::COPIES,
                "CPU path draws 4 mirror bodies plus 4 held bows"
            );
            // Verify the 4 held bows sit at the 4 scattered mirror positions (not at the real body).
            let mirror_bows = &out[body_verts * illusioner::COPIES..];
            for (i, chunk) in mirror_bows.chunks_exact(bow_verts).enumerate() {
                let n = bow_verts as f64;
                let s = chunk.iter().fold([0.0; 3], |a, v| {
                    [
                        a[0] + f64::from(v.pos[0]),
                        a[1] + f64::from(v.pos[1]),
                        a[2] + f64::from(v.pos[2]),
                    ]
                });
                let center = [s[0] / n, s[1] / n, s[2] / n];
                for k in 0..3 {
                    let want = bow_center[k] + offsets[i][k] + want_wobble[i][k];
                    assert!(
                        (center[k] - want).abs() < 1e-4,
                        "mirror bow {i} axis {k}: got {}, want {want}",
                        center[k]
                    );
                }
            }
        } else {
            // With `LivingFrame.skinning = true`, the 4 mirror bodies still fall back to CPU
            // (`posable = false`) while only the 4 held bows follow the skinned-item path.
            assert_eq!(
                skinned_jobs,
                bow_skin_jobs * illusioner::COPIES,
                "invisible illusioner bodies must stay off GPU skinning"
            );
            assert_eq!(
                out.len(),
                (body_verts + bow_cpu_verts_when_skinned) * illusioner::COPIES,
                "skinning=true draws 4 CPU mirror bodies plus expected mirror bow vertices"
            );
        }
        // First 4 * body_verts are the four CPU mirror bodies at the exact independent offsets.
        let mirror_bodies = &out[..body_verts * illusioner::COPIES];
        assert!(
            mirror_bodies.iter().all(|v| v.color >> 24 == 0xFF),
            "mirrors draw opaque"
        );
        for (i, chunk) in mirror_bodies.chunks_exact(body_verts).enumerate() {
            let n = body_verts as f64;
            let s = chunk.iter().fold([0.0; 3], |a, v| {
                [
                    a[0] + f64::from(v.pos[0]),
                    a[1] + f64::from(v.pos[1]),
                    a[2] + f64::from(v.pos[2]),
                ]
            });
            let center = [s[0] / n, s[1] / n, s[2] / n];
            for k in 0..3 {
                let want = body_center[k] + offsets[i][k] + want_wobble[i][k];
                assert!(
                    (center[k] - want).abs() < 1e-4,
                    "skinning={skinning} mirror {i} axis {k}: got {}, want {want}",
                    center[k]
                );
            }
        }
    }

    // Glowing + invisible: keeps all 4 mirrors AND records outlines for all 4.
    game.client.glowing_vertices.borrow_mut().clear();
    game.mobs[0].glowing = true;
    game.mobs[0].extra.set_main_hand_stack(None);
    let mut glow_out = Vec::new();
    game.mob_geometry(camera, alpha, &mut glow_out);
    assert_eq!(glow_out.len(), body_verts * illusioner::COPIES);
    assert_eq!(
        game.client.glowing_vertices.borrow().len(),
        body_verts * illusioner::COPIES
    );

    // Spectator view: illusioner mirrors stay opaque (0xFF), not forced translucent (0x26).
    game.mobs[0].glowing = false;
    set_mode(&mut game, GameMode::Spectator);
    let mut spec_out = Vec::new();
    game.mob_geometry(camera, alpha, &mut spec_out);
    assert_eq!(spec_out.len(), body_verts * illusioner::COPIES);
    assert!(spec_out.iter().all(|v| v.color >> 24 == 0xFF));
}
