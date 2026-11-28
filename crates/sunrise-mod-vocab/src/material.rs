//! Material and form vocabulary: the ids and convention tags of one material's family.
//!
//! A GregTech-era mod declares a material once and then registers every shape it takes: steel has an
//! ingot, a nugget, a dust, a rod, an ore and a storage block. In 1.7.10 those names lived in the ore
//! dictionary (`ingotSteel`, `dustSteel`, `blockSteel`, `oreSteel`); in Sunrise every variant is a
//! normal registry id, and a `c:` convention tag holds the family together. [`Form`] names the shapes
//! both sides share and derives the id and the tag for a material.
//!
//! The ids follow vanilla's shape: `steel_ingot`, `steel_nugget`, `steel_dust`, `steel_rod`,
//! `steel_ore`, `steel_block`, `raw_steel`; a gem is the material's own id (`ruby`). They are
//! suggestions for new content, not a rule vanilla follows: vanilla's gem is `amethyst_shard`, and a
//! suggestion that collides with another form's gets an explicit id from the mod. Every form in
//! [`Form::ALL`] has a **parent** tag Sunrise already ships under [`tags::c`](crate::tags::c); the
//! **child** tag `c:ingots/steel` is the mod's to define. A shape without a shipped tag (GT's plates
//! and gears, for instance) is the mod's own tag, not a [`Form`].

use std::fmt;

/// The shape a material takes when a mod registers it as an item or a block.
///
/// These are exactly the shapes Sunrise ships a `c:` parent tag for. The enum is non-exhaustive so a
/// later form can join without breaking a caller; [`Self::ALL`] lists today's forms.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, sunrise_mod_api_macros::Wire,
)]
#[non_exhaustive]
pub enum Form {
    /// A smelted bar: suggested id `steel_ingot`, parent tag `c:ingots`.
    Ingot,
    /// A small piece: suggested id `steel_nugget`, parent tag `c:nuggets`.
    Nugget,
    /// A ground pile: suggested id `steel_dust`, parent tag `c:dusts`.
    Dust,
    /// A cut gem, suggested under the material's own id: `ruby`, parent tag `c:gems`.
    Gem,
    /// A thin bar: suggested id `steel_rod`, parent tag `c:rods`.
    Rod,
    /// An unmelted piece: suggested id `raw_steel`, parent tag `c:raw_materials`.
    RawMaterial,
    /// An ore block: suggested id `steel_ore`, parent tag `c:ores`.
    Ore,
    /// A storage block: suggested id `steel_block`, parent tag `c:storage_blocks`.
    StorageBlock,
}

impl Form {
    /// Every form, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Ingot,
        Self::Nugget,
        Self::Dust,
        Self::Gem,
        Self::Rod,
        Self::RawMaterial,
        Self::Ore,
        Self::StorageBlock,
    ];

    /// Whether the form registers a block rather than an item, so its id names a block and its
    /// convention tag is a block tag.
    pub fn is_block(self) -> bool {
        matches!(self, Self::Ore | Self::StorageBlock)
    }

    /// The directory under `data/<namespace>/tags/` that holds the form's convention tag: `block`
    /// for an ore or storage block, `item` for every other form.
    pub fn tag_folder(self) -> &'static str {
        if self.is_block() { "block" } else { "item" }
    }

    /// The parent `c:` convention tag every variant of the form joins: `c:ingots` for every ingot.
    ///
    /// The parent is a tag Sunrise ships. A mod adds a child (`c:ingots/steel`) and, when recipes
    /// should see the new material through the parent too, appends that child to the parent's value
    /// list. A block form's parent is a block tag; the shipped `c:ores` and `c:storage_blocks` also
    /// exist in the item registry, for the block's item.
    pub fn convention_tag(self) -> &'static str {
        match self {
            Self::Ingot => "c:ingots",
            Self::Nugget => "c:nuggets",
            Self::Dust => "c:dusts",
            Self::Gem => "c:gems",
            Self::Rod => "c:rods",
            Self::RawMaterial => "c:raw_materials",
            Self::Ore => "c:ores",
            Self::StorageBlock => "c:storage_blocks",
        }
    }

    /// The id the form suggests for `material`, without a namespace: `steel_ingot`, `raw_steel`, or
    /// `steel` for a gem. Give it to `Registrar::item_key` or `Registrar::block_key` with the mod's
    /// namespace, or register an explicit id when the suggestion is not what the mod wants.
    ///
    /// The suggestion is a naming convention, not a registry rule: `material` may be any legal
    /// resource path, including `1steel` and `steel-ingot`. New content is conventionally snake_case
    /// (`wrought_iron`, never `WroughtIron`), which [`is_snake_case`] reports without enforcing.
    ///
    /// A suggestion can collide, because two forms and two materials can spell the same id: an ingot
    /// of `raw_steel` and a raw form of `steel_ingot` both read `raw_steel_ingot`, and a gem of
    /// `steel_ingot` reads `steel_ingot`. A caller in that position registers its own id and keeps
    /// [`Self::tag`] for the convention tag, so the material stays in its family.
    pub fn id(self, material: &str) -> Result<String, MaterialError> {
        check_material(material)?;
        Ok(self.material_id(material))
    }

    /// The child convention tag for `material`: `c:ingots/steel`. An ingot and a dust of the same
    /// material share `c:ingots`/`c:dusts` but not this name, so a recipe can ask for either.
    ///
    /// The child is the mod's to define under [`Self::tag_folder`]. A block form's tag is a block
    /// tag; the block's item joins the same name in the item registry when recipes should see the
    /// item, which is where the shipped item parents `c:ores` and `c:storage_blocks` live.
    pub fn tag(self, material: &str) -> Result<String, MaterialError> {
        check_material(material)?;
        Ok(self.material_tag(material))
    }

    /// [`Self::id`] for a material already checked.
    fn material_id(self, material: &str) -> String {
        match self {
            // A gem is the material itself: `ruby`, `sapphire`, `diamond`.
            Self::Gem => material.to_owned(),
            // A raw piece reads before the material, as vanilla's `raw_iron` does.
            Self::RawMaterial => format!("raw_{material}"),
            Self::Ingot => format!("{material}_ingot"),
            Self::Nugget => format!("{material}_nugget"),
            Self::Dust => format!("{material}_dust"),
            Self::Rod => format!("{material}_rod"),
            Self::Ore => format!("{material}_ore"),
            Self::StorageBlock => format!("{material}_block"),
        }
    }

    /// [`Self::tag`] for a material already checked.
    fn material_tag(self, material: &str) -> String {
        let form = self.convention_tag().trim_start_matches("c:");
        format!("c:{form}/{material}")
    }
}

/// Whether `name` is lower-case snake_case: `wrought_iron`, `plutonium244`.
///
/// This is the convention [`Form::id`] suggests for new content, not a registry rule: [`Form::id`]
/// and [`Form::tag`] accept every legal resource path, including `1steel` and `steel-ingot`. Report
/// a name with this when a mod wants its own content to read like the rest of the family, without
/// refusing a legal path a port already uses.
pub fn is_snake_case(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Why a material name was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// no wire: a registration-time diagnostic the caller renders, not a transported value.
#[non_exhaustive]
pub enum MaterialError {
    /// The name is empty.
    Empty,
    /// The name is not a legal resource path: it has a character outside `a-z`, `0-9`, `_`, `-` and
    /// `.`, or a path separator such as `/` or `:`.
    NotResourcePath,
}

impl fmt::Display for MaterialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "a material name is empty",
            Self::NotResourcePath => "a material name is not a legal resource path",
        })
    }
}

impl std::error::Error for MaterialError {}

/// Refuse a material that is not one legal resource path, so an id cannot smuggle in a namespace or
/// a space. The narrower snake_case convention is [`is_snake_case`] and never gates this.
fn check_material(material: &str) -> Result<(), MaterialError> {
    if material.is_empty() {
        return Err(MaterialError::Empty);
    }
    if material
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
    {
        Ok(())
    } else {
        Err(MaterialError::NotResourcePath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_material_takes_the_shape_vanilla_gives_it() {
        assert_eq!(Form::Ingot.id("steel").unwrap(), "steel_ingot");
        assert_eq!(Form::Nugget.id("steel").unwrap(), "steel_nugget");
        assert_eq!(Form::Dust.id("steel").unwrap(), "steel_dust");
        assert_eq!(Form::Rod.id("steel").unwrap(), "steel_rod");
        assert_eq!(Form::Ore.id("steel").unwrap(), "steel_ore");
        assert_eq!(Form::StorageBlock.id("steel").unwrap(), "steel_block");
        // A raw piece reads before the material, and a gem is the material's own id.
        assert_eq!(Form::RawMaterial.id("steel").unwrap(), "raw_steel");
        assert_eq!(Form::Gem.id("ruby").unwrap(), "ruby");
        // The suggestion is not vanilla's exact name: vanilla's gem is `amethyst_shard`.
        assert_eq!(Form::Gem.id("amethyst").unwrap(), "amethyst");
        assert_eq!(
            Form::ALL.map(|form| form.tag("steel").unwrap()),
            [
                "c:ingots/steel",
                "c:nuggets/steel",
                "c:dusts/steel",
                "c:gems/steel",
                "c:rods/steel",
                "c:raw_materials/steel",
                "c:ores/steel",
                "c:storage_blocks/steel",
            ]
        );
    }

    #[test]
    fn a_suggested_id_can_collide_so_the_mod_names_its_own() {
        // An ingot of `raw_steel` and a raw form of `steel_ingot` spell the same suggested id.
        assert_eq!(Form::Ingot.id("raw_steel").unwrap(), "raw_steel_ingot");
        assert_eq!(
            Form::RawMaterial.id("steel_ingot").unwrap(),
            "raw_steel_ingot"
        );
        // A gem of `steel_ingot` spells the same suggested id as an ingot of `steel`.
        assert_eq!(Form::Gem.id("steel_ingot").unwrap(), "steel_ingot");
        assert_eq!(Form::Ingot.id("steel").unwrap(), "steel_ingot");
        // The convention tags stay distinct, so the material keeps its family with an explicit id.
        assert_eq!(Form::Ingot.tag("raw_steel").unwrap(), "c:ingots/raw_steel");
        assert_eq!(
            Form::RawMaterial.tag("steel_ingot").unwrap(),
            "c:raw_materials/steel_ingot"
        );
    }

    #[test]
    fn accepts_every_legal_resource_path() {
        // `1steel` and `steel-ingot` are legal registry paths, so the helper builds them.
        assert_eq!(Form::Ingot.id("1steel").unwrap(), "1steel_ingot");
        assert_eq!(Form::Ingot.id("steel-ingot").unwrap(), "steel-ingot_ingot");
        assert_eq!(Form::Ingot.id("steel.ingot").unwrap(), "steel.ingot_ingot");
        assert_eq!(Form::Ingot.id("steel_2").unwrap(), "steel_2_ingot");
        assert_eq!(Form::Gem.tag("1steel").unwrap(), "c:gems/1steel");
        assert_eq!(Form::Ore.tag("steel-ingot").unwrap(), "c:ores/steel-ingot");
    }

    #[test]
    fn refuses_a_material_that_is_not_a_resource_path() {
        assert_eq!(Form::Ingot.id(""), Err(MaterialError::Empty));
        for name in [
            "Steel",
            "steel ingot",
            "steel:ingot",
            "my/steel",
            "steel#tag",
            "steel\u{e9}",
        ] {
            assert_eq!(
                Form::Ingot.id(name),
                Err(MaterialError::NotResourcePath),
                "{name}"
            );
            assert_eq!(
                Form::Ingot.tag(name),
                Err(MaterialError::NotResourcePath),
                "{name}"
            );
        }
    }

    #[test]
    fn snake_case_is_a_convention_not_a_registry_rule() {
        assert!(is_snake_case("wrought_iron"));
        assert!(is_snake_case("plutonium244"));
        assert!(!is_snake_case("1steel"));
        assert!(!is_snake_case("steel-ingot"));
        assert!(!is_snake_case("Steel"));
        assert!(!is_snake_case(""));
        // A name outside the convention still builds; the two checks serve different purposes.
        assert!(Form::Ingot.id("1steel").is_ok());
        assert!(!is_snake_case("1steel"));
    }

    #[test]
    fn every_form_has_a_distinct_id_and_tag() {
        let mut ids: Vec<_> = Form::ALL
            .iter()
            .map(|form| form.id("steel").unwrap())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), Form::ALL.len());

        let mut tags: Vec<_> = Form::ALL.iter().map(|form| form.convention_tag()).collect();
        tags.sort();
        tags.dedup();
        assert_eq!(tags.len(), Form::ALL.len());
        // Ores and storage blocks are blocks, so their ids and tags are block-registry names.
        assert_eq!(Form::Ore.tag_folder(), "block");
        assert_eq!(Form::StorageBlock.tag_folder(), "block");
        assert!(
            Form::ALL
                .iter()
                .all(|form| form.is_block() == matches!(form, Form::Ore | Form::StorageBlock))
        );
        assert_eq!(Form::Ingot.tag_folder(), "item");
    }
}
