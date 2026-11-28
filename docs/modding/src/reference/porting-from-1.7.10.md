# Porting a 1.7.10 mod to Sunrise

Sunrise does not load 1.7.10 jars. A port produces a `.smod` package: a content mod, a native Rust mod, or one
native mod that declares content and code together. The original's Java classes, Forge events and ASM patches
have no binary equivalent, so every port is a rewrite against the mod API, even a small one.

This page maps what 1.7.10 mods are built from to the Sunrise API that replaces it. Start with
[How a mod works](../concepts/how-mods-work.md) and [The manifest and the package](manifest.md) for the model;
come back here for the mapping and the clean-room rules.

## Choose the route first

| Your 1.7.10 mod is mostly | Port it as | Start at |
| --- | --- | --- |
| Blocks, items, fluids, tags, recipes, loot, worldgen data | Content mod (`data/` and `assets/`, no Rust) | [Your first content mod](../start/first-content-mod.md) |
| Behaviour: ticking, inventories, GUIs, events, custom rendering | Native mod (`impl Hooks`) | [Your first native mod](../start/first-mod.md) |
| A machine or structure with data-driven content *and* code | One native mod, with its `data/` and `assets/` beside `src/` | [Your first native mod](../start/first-mod.md) |

A native mod carries its own `data/` and `assets/` exactly as a content mod does; no manifest flag is needed for
that. `[native] serves_content = true` and a top-level `served_by` are only for a *separate* content package that
delegates its namespaces to a native runtime, such as a script or bytecode host that generates packages: both
packages opt in, and the content layer loads after its host. Any installed content package can name an opting-in runtime; the runtime decides which packages it understands. Unless you are writing such a host, keep one package.

A content mod describes what exists; a native mod decides what happens. Content declarations are checked and
admitted atomically, and a server refuses a join when the two sides' both-side mods differ, so keep the mod id
and version stable across the port.

## The mapping

The left column is the 1.7.10 and Forge API. The right column is the Sunrise route. Read the linked page before
writing code: identifiers, hook names and event fields are checked against the pinned build by
`cargo sunrise-mod check`.

### The mod itself

| 1.7.10 / Forge | Sunrise |
| --- | --- |
| `@Mod`, `mcmod.info`, `FMLPreInitializationEvent` | `sunrise-mod.toml` (`id`, `name`, `version`, `side`) plus one `impl Hooks` in `src/lib.rs`, marked `#[mod_api::sunrise_mod]`. See [The manifest and the package](manifest.md). |
| `@Mod.EventHandler` init, post-init, server-starting | `Hooks::register` for declarations and parts, then the load phases in [Load phases and access](../concepts/load-phases.md). |
| `@SideOnly(Side.CLIENT)`, client and common proxies | The manifest's `side` decides what a joining client must have. Hooks and parts declare their own side; a client hook receives a `ClientHost`, so the wrong side does not compile. See [Hooks and the Host](../concepts/hooks-and-the-host.md). |
| `Configuration` / `ForgeConfiguration` | `register_config` with a `ConfigSpec`; a `ConfigListener` receives a `ConfigSnapshot`. See [Config files and settings](../how-to/mod-settings.md). |
| `FMLInterModComms`, hard `@Mod` dependencies | `[dependencies]` declares a requirement and its load order: a required entry must be installed at a matching version, and by default its content and dispatch order come first. Load order is not listener priority. It sends no messages. For inter-mod communication, call the library's published functions and use its typed mod events (`#[subscribe]`, `Registrar::add_listener`). See [Dependencies between mods](../concepts/dependencies.md) and [Events between mods](../concepts/mod-events.md). |
| A coremod, `IClassTransformer`, a Mixin class | A `[[native.mixin]]` row: name the function, the place in it and a small Rust helper. See [Mixins](../concepts/native-route/2-mixins.md). Use a hook or a Part first when one exposes the same point. |

### Content and registries

| 1.7.10 / Forge | Sunrise |
| --- | --- |
| `GameRegistry.registerBlock` / `registerItem` | `data/<id>/sunrise/block/<path>.json` and `data/<id>/sunrise/item/<path>.json`, each copying a vanilla template. Code mods use `BlocksEvent` and `ItemsEvent` with `mod_api::Block::new` / `mod_api::Item::new`. See [Content mods](../how-to/content-mods.md). |
| `OreDictionary.registerOre` and `getOres` | Item, block and fluid tags under `data/<ns>/tags/{item,block,fluid}/`; request one with `TagsRegistrar::tag_key`, attach parts to every member with `attach_item_part_tag` / `attach_block_part_tag`, and read membership through `Host::tags`. See [Item and tag parts](../concepts/parts/item-and-tag-parts.md). |
| `metadata 0..15` on a block | Typed block states: declare the properties on the block and use `BlockState` plus `BlockMetadata` instead of a numeric metadata byte. See [Registry keys](../concepts/registry-keys.md). |
| Item damage / metadata as a variant | Typed data components on the stack, not a metadata word: read with `stack.get(&component)` and write with `set` or `remove`. `damage_component()` returns the stored numeric override, including zero, or `None`, never a variant or metadata id. See [Owned item values](../how-to/items/item-stack-values.md). |
| `Block`, `BlockContainer`, `ITickable` | A block declaration plus `BlockPart` (or `BlockEntityPart`) methods, and a block entity type with a ticker selector when it ticks. |
| `TileEntity`, `readFromNBT` / `writeToNBT` | A block entity type and a `BlockEntityPart`: `block_entity_save_additional` writes into a `mod_api::Compound` copy of the stored payload, and `block_entity_load_additional` refreshes live state after a payload is applied. `BlockEntityPartState` is attachment-backed: declare `.serialize(codec)` when the value must persist across a reload. See [Block entity types](../concepts/parts/block-entity-types.md) and [Attachments](../concepts/attachments.md). |
| `IInventory`, `ISidedInventory`, hoppers and pipes | Item capability `ITEMS` offered by a block entity or block provider, with per-face addresses. See [Capabilities](../concepts/capabilities.md). |
| `IFluidHandler`, tanks | Fluid capability (`FLUIDS`) with attached storage. See [Block fluids](../concepts/capabilities/block-fluids.md). |
| `IEnergyStorage`, GT EU buffers | `EnergyHandler` behind the `ENERGY` capability, with `AttachedEnergy` for engine-managed buffers. See [Block energy](../concepts/capabilities/block-energy.md). |
| `EntityRegistry.registerModEntity` | An entity type (`data/<id>/sunrise/entity_type/`) and an `EntityPart`; vanilla AI goals are parameterised rather than rewritten. See [Entity parts and mob goals](../concepts/parts/entity-parts-and-mob-goals.md). |
| `IExtendedEntityProperties` | Attachments: declare `AttachmentDescriptor::new(id, default)`, keep the `AttachmentKey`, and read or write through `Holder`. See [Attachments](../concepts/attachments.md). |
| `Container`, `GuiContainer`, `GuiScreen` | A menu type, `MenusRegistrar::register_menu_screen`, a `MenuPart` and slots; screens are built with the GUI APIs. See [Menus and containers](../concepts/capabilities/menu-slots.md). |
| `TileEntitySpecialRenderer`, `RenderLivingEvent` | Client hooks for block entity and entity rendering; dynamic models, particles and overlays live in [Client, rendering and sound](../how-to/client.md). |
| `SimpleNetworkWrapper` channels | Typed payloads registered with `PayloadsRegistrar`, dispatched on the side that registered a handler. See [Custom networking](../concepts/wire-types.md). |
| `WorldGenMinable`, `WorldGenerator`, `BiomeGenBase` | Feature types, placement modifiers, biome modifiers, structures and spawn placements. See [World generation](../how-to/world.md). |
| `DimensionManager`, portals | A dimension registration with `DimensionSettings`, a chunk generator or biome source, and portal shapes. See [Levels, dimensions and blocks](../how-to/world.md). |
| `GameRegistry.addRecipe`, `CraftingManager` changes | Recipe JSON under `data/<ns>/recipe/`; custom recipe types through `register_recipe_type`. Removals and input/output swaps use `data/<ns>/recipe_filter/*.json`. See [Recipe filters](../how-to/items/recipe-rules.md). |
| Loot pools and drops in code | Loot tables under `data/<ns>/loot_table/` and global loot modifiers. See [Items, recipes and loot](../how-to/items.md). |

## What has no equivalent yet

A port that needs one of these is blocked by a gap, not by a missing wrapper. Do not work around it with a
private copy of engine code; say which capability the port needs and why.

- **Multiblock validation.** There is no StructureLib-style pattern framework: a multiblock reads its
  surrounding block states and its hatches' capabilities itself. Keep the pattern in your own mod, read states
  through the level view, and treat a hatch as the capability it offers.
- **A quest engine and full NEI/WAILA-class lookup.** Recipe and block lookup hooks exist, but a questbook
  engine and a complete recipe-viewer panel parity do not.
- **Arbitrary coremod patching.** `[[native.mixin]]` covers named function sites. A patch that rewrites bytecode,
  adds a field to a vanilla class or replaces a class loader has no route.
- **Per-dimension gravity.** Entity gravity is engine behaviour, not a dimension setting; a space mod's custom
  gravity needs an engine change rather than a port-side hook.

## Clean-room and licence rules

These rules decide whether a port can be accepted, independently of whether it works. A port in your own project
answers to the terms of whatever it reuses from the original; a contribution to Sunrise also answers to the repository's rule, that only
original code or code whose licence allows its use may be read and used
([CONTRIBUTING.md](https://github.com/sunrise-org/sunrise/blob/main/CONTRIBUTING.md#only-original-code-or-code-whose-license-allows-it)).

- **Never copy Mojang code, assets or data files.** Sunrise ships no game files. The player supplies prepared
  resources; a mod never downloads them.
- **Read no mod code.** Code under a permissive licence (MIT, BSD, ISC, zlib, Apache-2.0, Unlicense, CC0) may be
  read, copied or adapted when its licence allows it. GPL, LGPL, AGPL, MPL, EPL, the Creative Commons licences,
  PolyForm, custom or source-available terms and all-rights-reserved projects are off limits even to read, so use
  their public documentation, their own data files and their observed behaviour. The mod porting pipeline is
  stricter: it reads no Minecraft mod's code at all.
- **Name and behaviour are facts; expression is not.** Reproducing "a tank holds 16 buckets" is fine.
  Reproducing the original's source layout, identifiers or comments is not.
- **Keep the original's assets external.** `cargo sunrise-mod use-jar` records an exact jar as a passive `assets/`
  and `data/` layer. It reads NeoForge (`META-INF/neoforge.mods.toml`) or Fabric (`fabric.mod.json`) metadata and
  refuses any jar without one, which includes a 1.7.10 jar whose only metadata is `mcmod.info`. A refused jar adds
  nothing to the package, so its assets stay out of the port: do not extract or copy them in by hand, and never
  commit the jar or its extracted files. See
  [Java jars as external assets](../how-to/content-mods.md#java-jars-as-external-assets).
- **Obligations follow the material actually reused.** Copied or adapted code, assets, models, textures, sounds,
  lang files or data tables carry their own terms: keep the copyright line and licence text, and add the notice
  entry `THIRD_PARTY_NOTICES.md` requires. Check the terms of the material you actually reuse; a licence that forbids
  derivative or commercial use still limits a port that reuses it.
- **Record what you ported from.** Fill the manifest's `license` and `license_url` for the port package itself, and
  credit and link the original separately in `credits`; keep the licence text of any reused material with your project. In a Sunrise pull request, add the
  `THIRD_PARTY_NOTICES.md` entry and say what you read, copied or adapted, with a link to the commit and the licence
  at that commit. `NOASSERTION` and `UNKNOWN` are not permission.
- **A manifest field or a registry entry is metadata, not permission.** The `license` field describes your package,
  and the registry's checks review its packaging and code, not whether you had the right to reuse the original.
  Publishing a port still needs permission, by licence or from the author, for whatever it reuses.
- **Data tables taken from the original** are reused material: check their terms.

## Checklist before you open a pull request

In your mod project:

```sh
cargo sunrise-mod check          # compiles and reports API usage against the pinned build
cargo test                       # the logic your hooks use, on the testing fakes
cargo sunrise-mod build          # optional: compiles against the prebuilt game slices
```

When the port itself lives in Sunrise (a registry entry, a shared library or engine work), also run the checks
that CI runs:

```sh
cargo fmt --all --check
cargo clippy -p <touched-crate> --all-targets --locked -- -D warnings
python3 tools/check_layers.py
python3 tools/ci/check_api_boundary.py --source-only   # when a public API surface changed
python3 tools/modding_concept_pages.py                 # page titles, links and includes
mdbook build docs/modding
git commit -s                                          # DCO: Signed-off-by, no Co-Authored-By
```

A content-only port needs no Rust build, but it still needs a manifest, namespaced files, and a test that the
package loads and its content registers. A port that changes a public API needs a test proving a mod can use it
end to end, not only a unit test of the new type.

## Order of work for a large port

1. **Inventory the gate list.** Write down every engine capability the mod needs: blocks, items, fluids, energy,
   menus, rendering, worldgen, networking. A missing one is a blocker to report, not something to fake.
2. **Port the data first.** Blocks, items, tags, recipes and loot get the mod loading and craftable before any
   code exists. A content mod is a valid first pull request.
3. **Port behaviour one Part at a time.** Ticking, then inventory, then menus, then rendering. Each Part is
   independent and testable on `testing::FakeHost`.
4. **Replace events with hooks, not with a dispatcher.** If a hook does not exist, name the NeoForge event it
   would mirror and file it; do not subscribe to a broader hook and filter by hand.
5. **Only then reach for a mixin.** A `[[native.mixin]]` is a patch to the game, so it needs the same review as
   engine code and an explanation of why a hook or Part cannot do it.
