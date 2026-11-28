#![cfg(test)]
use sunrise_gameplay_common::game::native_entity_refs_common::NativeEntityRefsCommonGameExt1 as _;

#[cfg(test)]
pub(crate) mod tests {
    use std::ops::ControlFlow;
    use sunrise_gameplay_common::game::api_values::TryIntoApi;
    use {
        crate::game::native_state_shapes::*,
        sunrise_gameplay_common::game::native_state_shapes::BlockShapeKind,
    };

    /// `BlockState.getVisualShape`: nothing for glass, iron bars and powder snow, a full
    /// block for soul sand and mud, the outline for a snow layer and a fence, the collision
    /// shape for any other block.
    #[test]
    #[ignore = "daily"]
    fn visual_shape_follows_vanilla_overrides() {
        sunrise_gameplay::game::native_entity_refs::tests::with_game(|game| {
            let level = game.level("minecraft:overworld").unwrap();
            for (block, expected) in [
                ("minecraft:glass", Vec::new()),
                ("minecraft:iron_bars", Vec::new()),
                ("minecraft:powder_snow", Vec::new()),
                ("minecraft:soul_sand", vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]),
                ("minecraft:mud", vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]),
                ("minecraft:snow", vec![[0.0, 0.0, 0.0, 1.0, 0.125, 1.0]]),
                (
                    "minecraft:oak_fence",
                    vec![[0.375, 0.0, 0.375, 0.625, 1.0, 0.625]],
                ),
                ("minecraft:stone_slab", vec![[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]]),
            ] {
                let state = game.pack.assets.registry.default_state(block).unwrap();
                let mut boxes = Vec::new();
                assert_eq!(
                    level.visit_block_shape(
                        game,
                        state.try_into_api().unwrap(),
                        [0, 64, 0],
                        BlockShapeKind::Visual,
                        &mut |bounds| {
                            boxes.push(bounds);
                            ControlFlow::Continue(())
                        }
                    ),
                    Some(ControlFlow::Continue(())),
                    "{block}"
                );
                assert_eq!(boxes, expected, "{block}");
            }
        });
    }

    #[test]
    #[ignore = "daily"]
    fn supplied_shape_state_does_not_replace_world_state() {
        sunrise_gameplay::game::native_entity_refs::tests::with_game(|game| {
            let level = game.level("minecraft:overworld").unwrap();
            let pos = [0, 64, 0];
            let stone = game
                .pack
                .assets
                .registry
                .default_state("minecraft:stone")
                .unwrap();
            let slab = game
                .pack
                .assets
                .registry
                .default_state("minecraft:stone_slab")
                .unwrap();
            let before = game.world.block(pos[0], pos[1], pos[2]);
            assert_ne!(before, stone);
            for kind in [BlockShapeKind::Outline, BlockShapeKind::Collision] {
                for (state, expected) in [
                    (stone, vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]),
                    (slab, vec![[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]]),
                    (before, Vec::new()),
                ] {
                    let mut boxes = Vec::new();
                    assert_eq!(
                        level.visit_block_shape(
                            game,
                            state.try_into_api().unwrap(),
                            pos,
                            kind,
                            &mut |bounds| {
                                boxes.push(bounds);
                                ControlFlow::Continue(())
                            }
                        ),
                        Some(ControlFlow::Continue(()))
                    );
                    assert_eq!(boxes, expected);
                }
            }
            assert_eq!(game.world.block(pos[0], pos[1], pos[2]), before);
        });
    }

    /// `CollisionGetter.isUnobstructed(state, pos, CollisionContext.empty())`: a standing armor
    /// stand blocks a full block, does not block a slab whose top is below the stand's feet, a
    /// marker stand blocks nothing, and a state with no collision shape is never obstructed. A
    /// parked level is read, a stale handle is `Err(Stale)` and a client is `Err(Unavailable)`.
    /// Runs in its own process: `with_game` publishes process-wide game tables.
    #[test]
    #[ignore = "daily"]
    fn is_unobstructed_tests_the_supplied_states_shape_against_building_entities() {
        sunrise_gameplay_common::game::in_own_process(
            "game::native_state_shapes::tests::is_unobstructed_tests_the_supplied_states_shape_against_building_entities",
            is_unobstructed_tests_the_supplied_states_shape_isolated,
        );
    }

    fn is_unobstructed_tests_the_supplied_states_shape_isolated() {
        use mod_api::LevelReadError;
        use sunrise_core::geometry::Vec3;
        sunrise_gameplay::game::native_entity_refs::tests::with_game(|game| {
            let level = game.level("minecraft:overworld").unwrap();
            let pos = [0, 64, 0];
            let state = |name: &str| {
                let id = game.pack.assets.registry.default_state(name).unwrap();
                id.try_into_api().unwrap()
            };
            let (stone, slab, air) = (
                state("minecraft:stone"),
                state("minecraft:stone_slab"),
                state("minecraft:air"),
            );
            for state in [stone, slab, air] {
                assert_eq!(level.is_unobstructed(game, pos, state), Ok(true));
            }
            // Feet at y 64.6: above the slab's top (64.5), inside the full block.
            game.armor_stands
                .push(sunrise_sim::entities::ArmorStand::new(
                    9101,
                    Vec3::new(0.5, 64.6, 0.5),
                    0.0,
                ));
            assert_eq!(level.is_unobstructed(game, pos, stone), Ok(false));
            assert_eq!(level.is_unobstructed(game, pos, slab), Ok(true));
            assert_eq!(level.is_unobstructed(game, pos, air), Ok(true));
            assert_eq!(level.is_unobstructed(game, [4, 64, 0], stone), Ok(true));
            // Feet at y 64.4: inside the slab's shape too.
            game.armor_stands.clear();
            game.armor_stands
                .push(sunrise_sim::entities::ArmorStand::new(
                    9102,
                    Vec3::new(0.5, 64.4, 0.5),
                    0.0,
                ));
            assert_eq!(level.is_unobstructed(game, pos, slab), Ok(false));
            // `Entity.blocksBuilding` is false for a marker armor stand.
            game.armor_stands.clear();
            let mut marker =
                sunrise_sim::entities::ArmorStand::new(9103, Vec3::new(0.5, 64.4, 0.5), 0.0);
            marker.marker = true;
            game.armor_stands.push(marker);
            assert_eq!(level.is_unobstructed(game, pos, stone), Ok(true));
            game.armor_stands.clear();

            let stale = mod_api::LevelRef::new(
                mod_api::WorldEpoch::new(level.epoch().get() + 1),
                level.dimension(),
            );
            assert_eq!(
                stale.is_unobstructed(game, pos, stone),
                Err(LevelReadError::Stale)
            );
            // The Overworld stays readable while parked, its blockers with it.
            game.armor_stands
                .push(sunrise_sim::entities::ArmorStand::new(
                    9104,
                    Vec3::new(0.5, 64.6, 0.5),
                    0.0,
                ));
            game.enter_level(sunrise_gameplay_common::game::Dim::Nether)
                .unwrap();
            assert_eq!(level.is_unobstructed(game, pos, stone), Ok(false));
            assert_eq!(level.is_unobstructed(game, [4, 64, 0], stone), Ok(true));
            game.options.execution_role =
                sunrise_gameplay_common::game::execution_role::ExecutionRole::Client;
            assert_eq!(
                level.is_unobstructed(game, pos, stone),
                Err(LevelReadError::Unavailable)
            );
        });
    }

    /// `ServerPlayer.die` leaves the player in the level: its body blocks building until
    /// `LivingEntity.tickDeath` removes it at death tick 20 (`EntityGetter.isUnobstructed`
    /// skips only removed entities and spectators).
    /// Runs in its own process: `with_game` publishes process-wide game tables.
    #[test]
    #[ignore = "daily"]
    fn is_unobstructed_counts_a_dead_guest_until_its_body_is_removed() {
        sunrise_gameplay_common::game::in_own_process(
            "game::native_state_shapes::tests::is_unobstructed_counts_a_dead_guest_until_its_body_is_removed",
            is_unobstructed_counts_a_dead_guest_isolated,
        );
    }

    fn is_unobstructed_counts_a_dead_guest_isolated() {
        sunrise_gameplay::game::native_entity_refs::tests::with_game(|game| {
            let level = game.level("minecraft:overworld").unwrap();
            let pos = [0, 64, 0];
            let stone = game
                .pack
                .assets
                .registry
                .default_state("minecraft:stone")
                .unwrap()
                .try_into_api()
                .unwrap();
            assert_eq!(level.is_unobstructed(game, pos, stone), Ok(true));
            sunrise_gameplay::game::native_player_death::tests::add_guest(game);
            fn set(game: &mut sunrise_gameplay::game::ServerGame<'_>, death_time: u32) {
                game.server
                    .lan
                    .as_mut()
                    .unwrap()
                    .hub
                    .guests
                    .update(0, |guest| {
                        guest.dimension = sunrise_gameplay_common::game::Dim::Overworld.id().into();
                        guest.position = [0.5, 64.0, 0.5];
                        guest.loaded = true;
                        guest.health = 0.0;
                        guest.dead = true;
                        guest.death_time = death_time;
                    });
            }
            for death_time in [0, 19] {
                set(game, death_time);
                assert_eq!(
                    level.is_unobstructed(game, pos, stone),
                    Ok(false),
                    "death tick {death_time}"
                );
            }
            set(game, 20);
            assert_eq!(level.is_unobstructed(game, pos, stone), Ok(true));
        });
    }
}
