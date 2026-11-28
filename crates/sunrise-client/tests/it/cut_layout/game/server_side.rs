pub(crate) mod native_entity_refs {
    pub(crate) mod tests {
        pub(crate) use sunrise_client::native_item_places_test_support::{
            with_game, with_resident_game,
        };
    }
}

pub(crate) mod native_guest_turn_tests {
    pub(crate) use sunrise_gameplay::game::native_guest_turn_tests::with_guests;
}

pub(crate) mod native_item_places;
