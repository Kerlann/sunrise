//! Server-side datapack vanilla-value table, tags, and obstruction queries.

use crate::game::ServerGame;
use mod_api::{
    BlockKey, BlockPredicateValue, BlockState, BlockStateProviderValue, DatapackValueError,
    HolderSetValue, IntProviderValue, LevelRef, RandomSource as ModRandomSource,
};
use std::collections::{HashMap, HashSet};
use sunrise_core::geometry::{Aabb, Vec3};
use sunrise_gameplay_common::game::Dim;
use sunrise_gameplay_common::game::native_datapack_values::{
    DatapackValuesSide, DatapackVanillaValues, GameDatapackValues,
    datapack_active_world_entities_unobstructed,
};
use sunrise_gameplay_common::game::native_level_refs_common::NativeLevelRefsCommonGameExt1 as _;
use sunrise_gameplay_common::game::native_state_shapes::try_collision_shape;
use sunrise_gameplay_common::game::tables::GameItems as _;
use sunrise_level::world::World;
use sunrise_world::registry::BlockStateId;
use sunrise_world::worldgen::placement::PlacementQueryError;

impl<'g> DatapackValuesSide<'g> for ServerGame<'g> {
    fn active_datapack_vanilla_values(&self) -> Option<&DatapackVanillaValues> {
        self.common.mods.datapack_vanilla_values.as_deref()
    }

    fn datapack_block_tags(&self) -> &HashMap<String, HashSet<String>> {
        &self.items().block_tags
    }

    /// `EntityGetter.isUnobstructed(null, Shapes.block().move(pos))`: the unit cell at
    /// `position` through [`ServerGame::is_shape_unobstructed`].
    fn is_datapack_block_unobstructed(
        &self,
        level: LevelRef,
        world: &World,
        position: [i32; 3],
    ) -> Result<bool, PlacementQueryError> {
        let cell = sunrise_core::geometry::VoxelShape::block().moved(Vec3::new(
            f64::from(position[0]),
            f64::from(position[1]),
            f64::from(position[2]),
        ));
        self.is_shape_unobstructed(level, world, &cell)
    }
}

impl ServerGame<'_> {
    /// `EntityGetter.isUnobstructed(null, shape)` for a world-space `shape`: checks every
    /// non-spectator player still in the level (including LAN guests and dedicated-server
    /// players, and a dead player until `LivingEntity.tickDeath` removes it) and every
    /// `blocksBuilding` entity in `level` (active or parked). Mobs come from the level's entity
    /// lookup by the shape's bounds, as `EntityGetter.getEntities(source, shape.bounds())` does,
    /// not from a walk over every mob. An empty shape is unobstructed. An overlap that cannot be
    /// evaluated (`Shapes.joinIsNotEmpty` over the grid limit) is `Err(Unavailable)`, not clear.
    pub(in crate::game) fn is_shape_unobstructed(
        &self,
        level: LevelRef,
        world: &World,
        shape: &sunrise_core::geometry::VoxelShape,
    ) -> Result<bool, PlacementQueryError> {
        let Some(cell) = shape.bounds() else {
            return Ok(true);
        };
        // A failed join stops the scan as a hit; `verdict` then reports the error.
        let failed = std::cell::Cell::new(false);
        let mut obstructs = |blocker: Aabb| {
            sunrise_world::entity_obstruction::shape_meets_box(shape, cell, blocker).unwrap_or_else(
                |_| {
                    failed.set(true);
                    true
                },
            )
        };
        let verdict = |obstructed: bool| {
            if failed.get() {
                Err(PlacementQueryError::Unavailable)
            } else {
                Ok(!obstructed)
            }
        };
        let dim_id = level.dimension_id();
        let dim = Dim::from_id(dim_id);
        let is_active = std::ptr::eq(world, &self.common.world);

        if is_active
            && !self.server.lan_acting_fake
            && !self.common.spectator()
            && self
                .common
                .player
                .as_ref()
                .is_some_and(|p| obstructs(p.bounds()))
        {
            return verdict(true);
        }
        if self.server.lan_acting.is_some() {
            if self
                .server
                .lan_building_boxes
                .0
                .iter(dim)
                .any(&mut obstructs)
            {
                return verdict(true);
            }
        } else if let Some(host) = self.server.lan.as_ref()
            && host
                .hub
                .guests
                .simulation_iter()
                .filter(|g| g.dimension == dim_id && g.loaded && g.blocks_building())
                .any(|g| obstructs(g.bounds()))
        {
            return verdict(true);
        }

        if is_active {
            let clear =
                datapack_active_world_entities_unobstructed(self, level, cell, obstructs, |_| {
                    false
                })?;
            return verdict(!clear);
        }

        if let Some(parked) = self.server.levels.0.parked_of(dim_id) {
            return verdict(parked.any_building_blocker(obstructs));
        }

        Ok(true)
    }
}

impl ServerGame<'_> {
    /// `CollisionGetter.isUnobstructed(state, pos, CollisionContext.empty())` in a server level:
    /// whether no `blocksBuilding` entity is in the supplied state's empty-context collision
    /// shape at `pos`. A shape that cannot be evaluated is `Err(Unavailable)`, not free.
    pub(in crate::game) fn level_is_unobstructed(
        &self,
        level: LevelRef,
        pos: [i32; 3],
        state: BlockStateId,
    ) -> Result<bool, mod_api::LevelReadError> {
        let world = self.native_level_world_result(level)?;
        let shape = try_collision_shape(self.common.collision, world, state, pos)
            .ok_or(mod_api::LevelReadError::Unavailable)?
            .moved(Vec3::new(
                f64::from(pos[0]),
                f64::from(pos[1]),
                f64::from(pos[2]),
            ));
        self.is_shape_unobstructed(level, world, &shape)
            .map_err(|_| mod_api::LevelReadError::Unavailable)
    }

    pub(in crate::game) fn datapack_int_provider_sample(
        &self,
        value: IntProviderValue,
        random: &mut dyn ModRandomSource,
    ) -> Result<i32, DatapackValueError> {
        GameDatapackValues::datapack_int_provider_sample(self, value, random)
    }

    pub(in crate::game) fn datapack_block_state_provider_state(
        &self,
        value: BlockStateProviderValue,
        level: LevelRef,
        random: &mut dyn ModRandomSource,
        pos: [i32; 3],
    ) -> Result<BlockState, DatapackValueError> {
        GameDatapackValues::datapack_block_state_provider_state(self, value, level, random, pos)
    }

    pub(in crate::game) fn datapack_block_predicate_test(
        &self,
        value: BlockPredicateValue,
        level: LevelRef,
        pos: [i32; 3],
    ) -> Result<bool, DatapackValueError> {
        GameDatapackValues::datapack_block_predicate_test(self, value, level, pos)
    }

    pub(in crate::game) fn datapack_holder_set_contains(
        &self,
        value: HolderSetValue,
        block: BlockKey,
    ) -> Result<bool, DatapackValueError> {
        GameDatapackValues::datapack_holder_set_contains(self, value, block)
    }
}
