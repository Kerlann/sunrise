//! `EntityRef` and `LivingRef` expose an unsigned `u32` id. The wire, NBT, and
//! signed engine fields such as a guest's `entity_id` carry the same bits as
//! `i32`. An id with the high bit set is negative in its signed representation,
//! not out of range (#6335, #6336).

/// Encode an unsigned entity id for the wire or a signed storage field,
/// preserving all 32 bits.
pub const fn wire_entity_id(id: u32) -> i32 {
    id as i32
}

/// Decode a wire or stored `i32` id to the unsigned id used by `EntityRef`,
/// preserving all 32 bits, including the sign bit.
pub const fn entity_id_bits(id: i32) -> u32 {
    id as u32
}

#[cfg(test)]
mod tests {
    use super::{entity_id_bits, wire_entity_id};

    #[test]
    fn the_storage_encoding_round_trips_a_high_bit_id() {
        let id = 0x8000_0001_u32;
        assert_eq!(wire_entity_id(id), -2_147_483_647);
        assert_eq!(entity_id_bits(wire_entity_id(id)), id);
        assert_eq!(entity_id_bits(-1), u32::MAX);
    }
}
