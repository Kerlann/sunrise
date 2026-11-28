# Client biome resolution and synced biome data-map reads

Synced clients can now resolve biomes and read synced biome data maps through `Host::biome`, `BiomeKey::view`, `BiomeKey::data`, and `BiomeView::data` (backed by `KeysHost::biome`, `KeysBackend::biome_view`, and `KeysBackend::biome_view_id` enabled on both server and client sides). Before a client receives its synchronized biome registry, these reads continue to return `BiomeKeyError::Unavailable` or `DataMapError::Unavailable`. Server-side behaviour, signatures, and hook versions are unchanged.
