# Materials and their forms

A GregTech-era mod declares a material once and then registers every shape it takes: steel has an
ingot, a nugget, a dust, a rod, an ore and a storage block. In 1.7.10 those shapes were ore-dictionary
names (`ingotSteel`, `dustSteel`, `blockSteel`), not registry entries. A port registers each shape as
a normal item or block, and `mod_api::Form` suggests both halves of the declaration: the registry id
and the `c:` convention tag that keeps the family together, so two ports of the same metal agree.

## Suggested ids and shipped tags

{{#include ../../examples/src/material.rs:family}}

| Form | Suggested id for `steel` | Parent tag | Id registers |
| --- | --- | --- | --- |
| `Form::Ingot` | `steel_ingot` | `c:ingots` | item |
| `Form::Nugget` | `steel_nugget` | `c:nuggets` | item |
| `Form::Dust` | `steel_dust` | `c:dusts` | item |
| `Form::Gem` | `steel` | `c:gems` | item |
| `Form::Rod` | `steel_rod` | `c:rods` | item |
| `Form::RawMaterial` | `raw_steel` | `c:raw_materials` | item |
| `Form::Ore` | `steel_ore` | `c:ores` | block |
| `Form::StorageBlock` | `steel_block` | `c:storage_blocks` | block |

The id is a **suggestion for new content**, not vanilla's exact naming. A gem reads as the material's
own id, so `Form::Gem.id("ruby")` is `ruby`; vanilla's gem is `amethyst_shard`, which a port names
explicitly. A raw piece reads before the material, as vanilla's `raw_iron` does.

The suggestion takes any legal resource path: `1steel` and `steel-ingot` build, because the registry
accepts them. New content is conventionally lower-case snake_case (`wrought_iron`, never
`WroughtIron`); `mod_api::is_snake_case` reports that convention but never gates `Form::id` or
`Form::tag`.

Every tag in the table is a **parent** tag Sunrise already ships (`mod_api::tags::c::items::INGOTS`
and its neighbours). A mod adds a **child** (`c:ingots/steel`) to it; the next section shows both
files. `Form::tag_folder` says where the child lives: `item` for an item form, `block` for an ore or a
storage block.

## Collisions and explicit ids

Two forms and two materials can spell the same suggested id. An ingot of `raw_steel` and a raw form
of `steel_ingot` both read `raw_steel_ingot`; a gem of `steel_ingot` reads `steel_ingot`, the same as
an ingot of `steel`. The suggestion is only a suggestion: when it collides, the mod registers an
explicit id and keeps `Form::tag` for the convention tag, so the material is still found by family.

{{#include ../../examples/src/material.rs:explicit}}

The function only requests the item key and builds the tag name. Registering the item under that key, and listing it in the child tag file, are separate steps.

The id and the tag are independent by design. `amethyst_gem` registers `mymod:amethyst_shard` and
`c:gems/amethyst`; a GE-era metal can register `mymod:steel_gem` and still join `c:gems/steel`.

## Which registry each name targets

An item form's id goes to `Registrar::item_key` and its child tag is an **item** tag. An ore or
storage block's id goes to `Registrar::block_key` and its child tag is a **block** tag. The shipped
`c:ores` and `c:storage_blocks` parents exist in both registries, for the block and for its block
item; when a recipe should match the item form of an ore or storage block, add the item membership
too (`data/c/tags/item/ores/steel.json`, or the storage-block equivalent). The item child also needs a line in the item parent (`#c:ores/steel` in `data/c/tags/item/ores.json`) when recipes query that parent.

## Declare the family

{{#include ../../examples/src/material.rs:declare}}

The id is namespaced by the mod that owns it, so the registrar sees `mymod:steel_ingot`. The tag keeps
the `c` namespace because it is a convention shared with other mods, not the mod's own tag.

## Tag files: a shipped parent and a mod child

Writing only the child file does not put the material in its family. A recipe that asks for
`#c:ingots/steel` sees the item, but a recipe that asks for `#c:ingots` does not: Sunrise ships
`c:ingots` with its own value list (`#c:ingots/copper`, `#c:ingots/iron`, …), and that file alone does
not mention steel.

Add both memberships, in the `c` tag namespace. First `data/c/tags/item/ingots/steel.json`, the child the mod defines:

```json
{
  "values": ["mymod:steel_ingot"]
}
```

Then `data/c/tags/item/ingots.json`, which appends to the parent Sunrise ships:

```json
{
  "values": ["#c:ingots/steel"]
}
```

A mod's tag file layers over the shipped one: its `values` are added unless the file sets
`"replace": true`, so the vanilla ingots stay in `c:ingots`. Do not copy the parent's value list into
the mod; add the one line that names the new child.

Another mod then asks for steel without naming yours. For a dynamic key,
`TagsRegistrar::tag_key("c:ingots/steel")` answers an untyped `TagKey`; bind it to the item registry
once with `.typed::<ItemKey>()` before an item query. The typed constants in `mod_api::tags::c::items`
need no binding.

A material with several mods' variants joins one child tag, which is what the ore dictionary was for.

## Porting a legacy name

The helper reads and writes registry names and convention tags; it does not parse ore-dictionary
names. Translate the legacy name by hand: `blockWroughtIron` is
`Form::StorageBlock.id("wrought_iron")` (`wrought_iron_block`) plus `.tag("wrought_iron")`
(`c:storage_blocks/wrought_iron`). A parser cannot tell `ingotHotSteel` (hot steel ingot) from a
material actually named `Hotstone` after the `ingot` prefix, so a legacy prefix that is not one of
the eight forms, and a size or state word between the form and the material, stays the port's
decision rather than a guess. Those shapes are the mod's own tags and ids, under its own namespace,
not a `c:` convention tag.

## What this does not do

It names ids and tags. It writes nothing to a registry, so the item still needs its model, its
translation key and its recipe, and the block still needs its blockstate and loot table. It also
does not place a material's variants in one creative tab or one recipe family: that is the mod's
registration code, using the ids this module suggests.
