use super::ServerGame;
use mod_api::WorldEpoch;
use sunrise_gameplay_common::game::Dim;
use sunrise_level::{world::World, worldborder::WorldBorder};
use sunrise_sim::{
    join_seq::JoinSeq,
    lan::guest::{Guest, GuestKind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LevelId {
    pub(super) dim: Dim,
    epoch: Option<WorldEpoch>,
    opened: (u64, u64),
}

impl LevelId {
    #[inline(always)]
    pub(super) fn loaded(game: &ServerGame<'_>, dim: Dim) -> Option<Self> {
        let levels = &game.server.levels.0;
        let loaded = if dim == game.common.dim {
            !levels.active_closed
        } else {
            levels.parked.contains_key(&dim)
        };
        if !loaded {
            return None;
        }
        Some(Self {
            dim,
            epoch: game.common.native_lifecycle.epoch(),
            opened: levels.incarnation(dim)?,
        })
    }

    #[inline(always)]
    fn current(self, game: &ServerGame<'_>) -> bool {
        Self::loaded(game, self.dim) == Some(self)
    }

    #[inline(always)]
    pub(super) fn view<'a>(self, game: &'a ServerGame<'_>) -> Option<LevelView<'a>> {
        (self.dim == game.common.dim && self.current(game)).then(|| LevelView::active(game))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlayerId {
    conn: u32,
    kind: GuestKind,
    join: JoinSeq,
}

impl PlayerId {
    #[inline(always)]
    pub(super) fn guest(guest: &Guest) -> Self {
        Self {
            conn: guest.conn,
            kind: guest.kind,
            join: guest.residency.join(),
        }
    }

    #[inline(always)]
    pub(super) fn view_at<'a>(
        self,
        game: &'a ServerGame<'_>,
        index: usize,
    ) -> Option<PlayerView<'a>> {
        let guest = game.server.lan.as_ref()?.hub.guests.get(index)?;
        (Self::guest(guest) == self
            && if guest.is_fake() {
                !guest.disconnect_pending && guest.left_reason.is_empty()
            } else {
                guest.play_connection_open()
            })
        .then_some(PlayerView { guest })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ActorInLevel {
    pub(super) player: PlayerId,
    pub(super) level: LevelId,
}

impl ActorInLevel {
    #[inline(always)]
    pub(super) fn view_at<'a>(
        self,
        game: &'a ServerGame<'_>,
        index: usize,
    ) -> Option<PlayerView<'a>> {
        self.level.view(game)?;
        let player = self.player.view_at(game, index)?;
        (player.guest.dimension == self.level.dim.id()).then_some(player)
    }
}

pub(super) struct LevelView<'a> {
    world: &'a World,
    border: &'a WorldBorder,
}

impl<'a> LevelView<'a> {
    #[inline(always)]
    pub(super) fn active(game: &'a ServerGame<'_>) -> Self {
        Self {
            world: &game.common.world,
            border: &game.common.border,
        }
    }

    #[inline(always)]
    pub(super) fn build_limits(&self) -> (i32, i32) {
        (self.world.shape.min_y, self.world.shape.max_y() - 1)
    }

    #[inline(always)]
    pub(super) fn contains(&self, x: f64, z: f64) -> bool {
        self.border.contains(x, z)
    }
}

pub(super) struct PlayerView<'a> {
    pub(super) guest: &'a Guest,
}

#[cfg(test)]
#[path = "state_seam/tests.rs"]
pub(super) mod tests;
