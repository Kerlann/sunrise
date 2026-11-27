use super::{Dim, ServerGame, commands::execute::Source};
use mod_api::WorldEpoch;
use sunrise_level::{world::World, worldborder::WorldBorder};
use sunrise_sim::{
    join_seq::JoinSeq,
    lan::guest::{Guest, GuestKind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LevelId {
    pub(super) dim: Dim,
    epoch: Option<WorldEpoch>,
    opened: u64,
}

impl LevelId {
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

    pub(super) fn next_open(game: &ServerGame<'_>, dim: Dim) -> Self {
        Self {
            dim,
            epoch: game.common.native_lifecycle.epoch(),
            opened: game.server.levels.0.next_incarnation(dim),
        }
    }

    pub(super) fn current(self, game: &ServerGame<'_>) -> bool {
        Self::loaded(game, self.dim) == Some(self)
    }

    pub(super) fn in_place(self, game: &ServerGame<'_>) -> bool {
        self.dim == game.common.dim && self.current(game)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlayerId {
    Owner {
        epoch: Option<WorldEpoch>,
        join: JoinSeq,
    },
    Guest {
        conn: u32,
        kind: GuestKind,
        join: JoinSeq,
    },
}

impl PlayerId {
    pub(super) fn guest(guest: &Guest) -> Self {
        Self::Guest {
            conn: guest.conn,
            kind: guest.kind,
            join: guest.residency.join(),
        }
    }

    pub(super) fn acting(game: &ServerGame<'_>) -> Option<Self> {
        if let Some(conn) = game.server.lan_acting {
            Some(Self::guest(game.server.lan.as_ref()?.hub.guest(conn)?))
        } else {
            game.common.player.as_ref()?;
            Some(Self::Owner {
                epoch: game.common.native_lifecycle.epoch(),
                join: game.common.resident_entities.local_player.join(),
            })
        }
    }

    pub(super) fn live_index(self, game: &ServerGame<'_>) -> Option<usize> {
        let Self::Guest { conn, .. } = self else {
            return None;
        };
        let guests = &game.server.lan.as_ref()?.hub.guests;
        let index = guests.connection_index(conn)?;
        let guest = guests.get(index)?;
        (Self::guest(guest) == self
            && if guest.is_fake() {
                !guest.disconnect_pending && guest.left_reason.is_empty()
            } else {
                guest.play_connection_open()
            })
        .then_some(index)
    }

    pub(super) fn index_in(self, game: &ServerGame<'_>, level: LevelId) -> Option<usize> {
        if !level.current(game) {
            return None;
        }
        let index = self.live_index(game)?;
        let guest = game.server.lan.as_ref()?.hub.guests.get(index)?;
        (guest.dimension == level.dim.id()).then_some(index)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ActorInLevel {
    pub(super) player: PlayerId,
    pub(super) level: LevelId,
}

impl ActorInLevel {
    pub(super) fn acting(game: &ServerGame<'_>) -> Option<Self> {
        Some(Self {
            player: PlayerId::acting(game)?,
            level: LevelId::loaded(game, game.common.dim)?,
        })
    }

    pub(super) fn index(self, game: &ServerGame<'_>) -> Option<usize> {
        self.player.index_in(game, self.level)
    }

    pub(super) fn in_place(self, game: &ServerGame<'_>) -> bool {
        self.level.dim == game.common.dim && self.index(game).is_some()
    }

    pub(super) fn still_acting(self, game: &ServerGame<'_>) -> bool {
        self.level.in_place(game)
            && PlayerId::acting(game) == Some(self.player)
            && match self.player {
                PlayerId::Owner { .. } => true,
                PlayerId::Guest { .. } => self.index(game).is_some(),
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CommandOrigin {
    player: Option<PlayerId>,
    level: Option<LevelId>,
    block: Option<(i32, i32, i32)>,
    cart: Option<u32>,
}

impl CommandOrigin {
    pub(super) fn capture(game: &ServerGame<'_>, source: &Source) -> Self {
        Self {
            player: source
                .origin_player
                .then(|| PlayerId::acting(game))
                .flatten(),
            level: LevelId::loaded(game, source.origin_dimension),
            block: source.block,
            cart: source.cart,
        }
    }
}

pub(super) struct LevelView<'a> {
    pub(super) world: &'a World,
    pub(super) border: &'a WorldBorder,
}

pub(super) struct PlayerView<'a> {
    pub(super) guest: &'a Guest,
}

#[cfg(test)]
#[path = "state_seam/tests.rs"]
mod tests;
