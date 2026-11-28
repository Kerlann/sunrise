# Materials and their forms

A GregTech-era mod declares a material once and then registers every shape it takes: steel has an
ingot, a nugget, a dust, a rod, an ore and a storage block. In 1.7.10 those shapes were ore-dictionary
names (`ingotSteel`, `dustSteel`, `blockSteel`), not registry entries. A port registers each shape as
a normal item or block, and `mod_api::Form` derives both halves of the declaration: the registry id
and the `c:` convention tag that keeps the family together, so two ports of the same metal agree.

## Ids and tags

{{#include ../../examples/src/material.rs:family}}

| Form | Id for `steel` | Convention tag | Registers |
| --- | --- | --- | --- |
| `Form::Ingot` | `steel_ingot` | `c:ingots` | item |
| `Form::Nugget` | `steel_nugget` | `c:nuggets` | item |
| `Form::Dust` | `steel_dust` | `c:dusts` | item |
| `Form::Gem` | `ruby` | `c:gems` | item |
| `Form::Rod` | `steel_rod` | `c:rods` | item |
| `Form::RawMaterial` | `raw_steel` | `c:raw_materials` | item |
| `Form::Ore` | `steel_ore` | `c:ores` | block |
| `Form::StorageBlock` | `steel_block` | `c:storage_blocks` | block |

A gem is the material's own id: `ruby`, not `ruby_gem`. A raw piece reads before the material, as
vanilla's `raw_iron` does. The material half is snake_case and is the same in every form, so
`Form::id` and `Form::tag` refuse `Steel`, `steel ingot` and `steel-ingot` instead of building an id
that cannot load.

Every tag in that table is one Sunrise already ships (`mod_api::tags::c::items::INGOTS` and its
neighbours), so a port adds membership instead of inventing a name. `Form::tag_folder` says where:
`item` for an item form, `block` for an ore or a storage block.

## Declare the family

{{#include ../../examples/src/material.rs:declare}}

The id is namespaced by the mod that owns it, so the registrar sees `mymod:steel_ingot`. The tag has
its own namespace: the tag named `c:ingots/steel` is the file `data/c/tags/item/ingots/steel.json`,
whose values list the ids that join it:

```json
{
  "values": ["mymod:steel_ingot"]
}
```

Another mod then asks for steel without naming yours: `TagKey<ItemKey>` from
`TagsRegistrar::tag_key("c:ingots/steel")`, a `#c:ingots/steel` ingredient in a recipe, or a block
tag for an ore. A material with several mods' variants joins one tag, which is what the ore
dictionary was for.

## Porting a legacy name

{{#include ../../examples/src/material.rs:legacy}}

`parse_ore_name` reads the name as `<form><Material>` and converts the CamelCase material to
snake_case, so `blockWroughtIron` becomes the id `wrought_iron_block` and the tag
`c:storage_blocks/wrought_iron`. `material_name` is that conversion on its own, for a material a
legacy mod names outside the ore dictionary.

A name whose shape has no shipped tag is refused rather than guessed. `plateSteel` and `gearSteel`
keep their prefix and read as `None`; so do `dustSmallIron`, `dustTinyIron` and `ingotHotSteel`,
where a size or state word sits between the form and the material. Those are the mod's own tags and
ids, under its own namespace, not a `c:` convention tag.

## What this does not do

It names ids and tags. It writes nothing to a registry, so the item still needs its model, its
translation key and its recipe, and the block still needs its blockstate and loot table. It also
does not place a material's variants in one creative tab or one recipe family: that is the mod's
registration code, using the ids this module derives.
