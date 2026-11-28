//! Entity obstruction against a voxel shape. Entity storage, lifecycle and
//! vehicle-tree maintenance supply the immutable candidate snapshots here.
use sunrise_core::geometry::{Aabb, ShapeError, ShapeOp, VoxelShape};

/// Object identity within the caller's entity snapshot, including its generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityIdentity(pub u64);
#[derive(Clone, Copy, Debug)]
pub struct ObstructionEntity {
    pub identity: EntityIdentity,
    pub root_vehicle: EntityIdentity,
    pub bounds: Aabb,
    pub removed: bool,
    pub spectator: bool,
    pub blocks_building: bool,
}
pub trait EntityObstructionWorld {
    type Error;
    /// Preserve native query order. Implementations may return a broad-phase
    /// superset: exclusion, spectator and exact AABB filters are enforced below.
    fn obstruction_candidates(
        &mut self,
        excluded: Option<EntityIdentity>,
        bounds: Aabb,
    ) -> Result<Vec<ObstructionEntity>, Self::Error>;
}
#[derive(Debug)]
pub enum ObstructionError<E> {
    World(E),
    Shape(ShapeError),
}
impl<E: std::fmt::Display> std::fmt::Display for ObstructionError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::World(e) => e.fmt(f),
            Self::Shape(e) => e.fmt(f),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for ObstructionError<E> {}

pub fn is_unobstructed<W: EntityObstructionWorld>(
    world: &mut W,
    excluded: Option<&ObstructionEntity>,
    shape: &VoxelShape,
) -> Result<bool, ObstructionError<W::Error>> {
    let Some(bounds) = shape.bounds() else {
        return Ok(true);
    };
    let candidates = world
        .obstruction_candidates(excluded.map(|e| e.identity), bounds)
        .map_err(ObstructionError::World)?;
    for entity in candidates {
        if excluded.is_some_and(|e| e.identity == entity.identity)
            || entity.spectator
            || !entity.bounds.intersects(bounds)
        {
            continue;
        }
        if entity.removed
            || !entity.blocks_building
            || excluded.is_some_and(|e| e.root_vehicle == entity.root_vehicle)
        {
            continue;
        }
        if shape_meets_box(shape, bounds, entity.bounds).map_err(ObstructionError::Shape)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The per-entity test of `EntityGetter.isUnobstructed`: whether `blocker` overlaps `shape`
/// (whose bounds are `bounds`), `Shapes.joinIsNotEmpty(shape, Shapes.create(blocker),
/// BooleanOp.AND)`. A join that cannot be evaluated (a merged grid over the size limit) is an
/// error, never clear.
pub fn shape_meets_box(
    shape: &VoxelShape,
    bounds: Aabb,
    blocker: Aabb,
) -> Result<bool, ShapeError> {
    if !blocker.intersects(bounds) {
        return Ok(false);
    }
    shape.join_is_not_empty(&VoxelShape::from_box(blocker)?, ShapeOp::AND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sunrise_core::geometry::Vec3;

    #[test]
    fn a_blocker_whose_join_exceeds_the_grid_limit_is_an_error_not_clear() {
        // 1024 x 1024 x 1 cells is the grid limit; the box's z faces at 0.25 and 0.75 split
        // the single z cell, so the merged grid is over the limit.
        let shape = VoxelShape::from_cube([1024, 1024, 1], &vec![true; 1024 * 1024]).unwrap();
        let bounds = shape.bounds().unwrap();
        let blocker = Aabb::new(Vec3::new(0.0, 0.0, 0.25), Vec3::new(1.0, 1.0, 0.75));
        assert!(shape_meets_box(&shape, bounds, blocker).is_err());
        // A blocker outside the bounds never reaches the join.
        let apart = Aabb::new(Vec3::new(2.0, 0.0, 0.25), Vec3::new(3.0, 1.0, 0.75));
        assert_eq!(shape_meets_box(&shape, bounds, apart), Ok(false));
        let cell = VoxelShape::block();
        let inside = Aabb::new(Vec3::new(0.2, 0.2, 0.2), Vec3::new(0.8, 0.8, 0.8));
        assert_eq!(
            shape_meets_box(&cell, cell.bounds().unwrap(), inside),
            Ok(true)
        );
    }
}
