//! Material and form vocabulary: the ids and convention tags of one material's family.
//!
//! A GregTech-era mod declares a material once and then registers every shape it takes: steel has an
//! ingot, a nugget, a dust, a rod, an ore and a storage block. In 1.7.10 those names lived in the ore
//! dictionary (`ingotSteel`, `dustSteel`, `blockSteel`, `oreSteel`); in Sunrise every variant is a
//! normal registry id, and a `c:` convention tag holds the family together. [`Form`] names the shapes
//! both sides share and derives the id and the tag for a material, and [`parse_ore_name`] reads a
//! legacy ore-dictionary name back into the form and material a port declares.
//!
//! The ids follow vanilla's shape: `steel_ingot`, `steel_nugget`, `steel_dust`, `steel_rod`,
//! `steel_ore`, `steel_block`, `raw_steel`; a gem is the material's own id (`ruby`). Every form in
//! [`Form::ALL`] has a tag Sunrise already ships under [`tags::c`](crate::tags::c), so a mod declares
//! membership instead of inventing the name. A shape without a shipped tag (GT's plates and gears,
//! for instance) is the mod's own tag, not a [`Form`].

use std::fmt;

/// The shape a material takes when a mod registers it as an item or a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Form {
    /// A smelted bar: `steel_ingot`, convention tag `c:ingots`.
    Ingot,
    /// A small piece: `steel_nugget`, convention tag `c:nuggets`.
    Nugget,
    /// A ground pile: `steel_dust`, convention tag `c:dusts`.
    Dust,
    /// A cut gem, registered under the material's own id: `ruby`, convention tag `c:gems`.
    Gem,
    /// A thin bar: `steel_rod`, convention tag `c:rods`.
    Rod,
    /// An unmelted piece: `raw_steel`, convention tag `c:raw_materials`.
    RawMaterial,
    /// An ore block: `steel_ore`, convention tag `c:ores`.
    Ore,
    /// A storage block: `steel_block`, convention tag `c:storage_blocks`.
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

    /// Whether the form registers a block rather than an item, so its tag is a block tag.
    pub fn is_block(self) -> bool {
        matches!(self, Self::Ore | Self::StorageBlock)
    }

    /// The directory under `data/<namespace>/tags/` that holds the form's convention tag.
    pub fn tag_folder(self) -> &'static str {
        if self.is_block() { "block" } else { "item" }
    }

    /// The `c:` convention tag every variant of the form joins: `c:ingots` for every ingot.
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

    /// The form's id for `material`, without a namespace: `steel_ingot`, `raw_steel`, or `ruby` for
    /// a gem. Give it to `Registrar::item_key` or `Registrar::block_key` with the mod's namespace.
    ///
    /// `material` is snake_case, because it is the half of the id the mod chooses once and every
    /// form reuses it: `wrought_iron`, never `WroughtIron`.
    pub fn id(self, material: &str) -> Result<String, MaterialError> {
        check_material(material)?;
        Ok(self.material_id(material))
    }

    /// The form's convention tag for `material`: `c:ingots/steel`. An ingot and a dust of the same
    /// material share `c:ingots`/`c:dusts` but not this name, so a recipe can ask for either.
    pub fn tag(self, material: &str) -> Result<String, MaterialError> {
        check_material(material)?;
        Ok(self.material_tag(material))
    }

    /// [`Self::id`] for a material already checked by [`parse_ore_name`].
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

    /// [`Self::tag`] for a material already checked by [`parse_ore_name`].
    fn material_tag(self, material: &str) -> String {
        let form = self.convention_tag().trim_start_matches("c:");
        format!("c:{form}/{material}")
    }
}

/// A legacy 1.7.10 ore-dictionary name, read as the form and material a port declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OreName {
    /// The shape the name spells.
    pub form: Form,
    /// The material in snake_case, as [`Form::id`] and [`Form::tag`] take it.
    pub material: String,
}

impl OreName {
    /// The id the port declares for this name: `steel_ingot`.
    pub fn id(&self) -> String {
        self.form.material_id(&self.material)
    }

    /// The convention tag the port adds the id to: `c:ingots/steel`.
    pub fn tag(&self) -> String {
        self.form.material_tag(&self.material)
    }
}

/// Why a material name was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MaterialError {
    /// The name is empty.
    Empty,
    /// The name is not lower-case snake_case: `Steel`, `steel ingot`, `steel-ingot`, `1steel`.
    NotSnakeCase,
}

impl fmt::Display for MaterialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "a material name is empty",
            Self::NotSnakeCase => "a material name is not lower-case snake_case",
        })
    }
}

impl std::error::Error for MaterialError {}

/// Read a 1.7.10 ore-dictionary name as the form and material it spells: `ingotSteel` is an
/// [`Form::Ingot`] of `steel`, and `blockWroughtIron` a [`Form::StorageBlock`] of `wrought_iron`.
///
/// The prefix is the name's leading lower-case run, matched whole. A shape with no shipped form is
/// `None` rather than a guess: `plateSteel` and `gearSteel` keep their prefix and are refused, and a
/// size or state word between the form and the material (`dustSmallIron`, `dustTinyIron`,
/// `ingotHotSteel`) is refused too instead of being read as a material that starts with `Small`,
/// `Tiny` or `Hot`. A prefix without a material (`ingot`) or a material with characters outside
/// `A-Za-z0-9` (`dustNaquadah-Enriched`) is `None` as well.
pub fn parse_ore_name(name: &str) -> Option<OreName> {
    // The prefix is the leading lower-case run; the material starts at the first other character.
    let split = name.find(|c: char| !c.is_ascii_lowercase())?;
    let (prefix, rest) = name.split_at(split);
    let form = match prefix {
        "ingot" => Form::Ingot,
        "nugget" => Form::Nugget,
        "dust" => Form::Dust,
        "gem" => Form::Gem,
        "rod" => Form::Rod,
        "raw" => Form::RawMaterial,
        "ore" => Form::Ore,
        "block" => Form::StorageBlock,
        _ => return None,
    };
    // A qualifier says the material is not the rest of the name, so this is not a `<form><Material>`
    // name at all: refuse it instead of declaring `small_iron` or `hot_steel`.
    const QUALIFIERS: [&str; 5] = ["Small", "Tiny", "Hot", "Dense", "Double"];
    if QUALIFIERS
        .iter()
        .any(|qualifier| rest.starts_with(qualifier))
    {
        return None;
    }
    let material = material_name(rest)?;
    check_material(&material).ok()?;
    Some(OreName { form, material })
}

/// The snake_case material a CamelCase legacy name spells: `WroughtIron` is `wrought_iron`,
/// `UUMatter` is `uu_matter`. `None` for a name with a character outside `A-Za-z0-9`, or one with no
/// letter at all.
///
/// This is the conversion [`parse_ore_name`] applies to the material half; call it directly for a
/// material a legacy mod names outside the ore dictionary.
pub fn material_name(name: &str) -> Option<String> {
    let mut out = String::with_capacity(name.len() + 4);
    let mut after_lower = false;
    for (index, c) in name.char_indices() {
        if c.is_ascii_uppercase() {
            let next_lower = name[index + 1..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_lowercase());
            // `Steel` opens the name, `aB` starts a word, and `UUMatter` starts one before the `M`.
            if index > 0 && (after_lower || next_lower) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            after_lower = false;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            after_lower = true;
        } else {
            return None;
        }
    }
    (!out.is_empty() && out.chars().any(|c| c.is_ascii_alphabetic())).then_some(out)
}

/// Refuse a material that is not snake_case, so an id cannot smuggle in a namespace or a space.
fn check_material(material: &str) -> Result<(), MaterialError> {
    if material.is_empty() {
        return Err(MaterialError::Empty);
    }
    let mut chars = material.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return Err(MaterialError::NotSnakeCase),
    }
    if material
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        Ok(())
    } else {
        Err(MaterialError::NotSnakeCase)
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
    fn a_legacy_ore_name_reads_back_to_the_same_declaration() {
        for (legacy, form, material) in [
            ("ingotSteel", Form::Ingot, "steel"),
            ("nuggetSteel", Form::Nugget, "steel"),
            ("dustNaquadahEnriched", Form::Dust, "naquadah_enriched"),
            ("gemRuby", Form::Gem, "ruby"),
            ("rodSteel", Form::Rod, "steel"),
            ("blockWroughtIron", Form::StorageBlock, "wrought_iron"),
            ("oreIron", Form::Ore, "iron"),
        ] {
            let ore = parse_ore_name(legacy).expect(legacy);
            assert_eq!(ore.form, form, "{legacy}");
            assert_eq!(ore.material, material, "{legacy}");
            assert_eq!(ore.id(), form.id(material).unwrap(), "{legacy}");
            assert_eq!(ore.tag(), form.tag(material).unwrap(), "{legacy}");
        }
        // The same declaration the mod writes directly, so a port and a new mod agree on the id.
        assert_eq!(
            parse_ore_name("ingotSteel").unwrap().id(),
            Form::Ingot.id("steel").unwrap()
        );
    }

    #[test]
    fn refuses_a_name_that_is_not_a_shipped_form() {
        // Shapes with no shipped convention tag, and a name with no material.
        assert_eq!(parse_ore_name("plateSteel"), None);
        assert_eq!(parse_ore_name("gearSteel"), None);
        assert_eq!(parse_ore_name("dustSmallIron"), None);
        assert_eq!(parse_ore_name("dustTinyIron"), None);
        assert_eq!(parse_ore_name("ingotHotSteel"), None);
        assert_eq!(parse_ore_name("stickWood"), None);
        assert_eq!(parse_ore_name("ingot"), None);
        assert_eq!(parse_ore_name("Steel"), None);
        assert_eq!(parse_ore_name("dustNaquadah-Enriched"), None);
        // The empty prefix is not a form, so a bare material is refused rather than half-read.
        assert_eq!(parse_ore_name(""), None);
    }

    #[test]
    fn camel_case_splits_acronyms_and_digits() {
        assert_eq!(material_name("Steel").unwrap(), "steel");
        assert_eq!(material_name("WroughtIron").unwrap(), "wrought_iron");
        assert_eq!(
            material_name("NaquadahEnriched").unwrap(),
            "naquadah_enriched"
        );
        assert_eq!(material_name("UUMatter").unwrap(), "uu_matter");
        assert_eq!(material_name("Plutonium244").unwrap(), "plutonium244");
        assert_eq!(material_name("").as_deref(), None);
        assert_eq!(material_name("244").as_deref(), None);
        assert_eq!(material_name("Steel_Ingot").as_deref(), None);
    }

    #[test]
    fn refuses_a_material_that_is_not_snake_case() {
        assert_eq!(Form::Ingot.id(""), Err(MaterialError::Empty));
        for name in [
            "Steel",
            "steel ingot",
            "steel-ingot",
            "1steel",
            "steel:ingot",
            "my/steel",
        ] {
            assert_eq!(
                Form::Ingot.id(name),
                Err(MaterialError::NotSnakeCase),
                "{name}"
            );
            assert_eq!(
                Form::Ingot.tag(name),
                Err(MaterialError::NotSnakeCase),
                "{name}"
            );
        }
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
        // Ores and storage blocks are blocks, so their tags are block tags.
        assert!(
            Form::ALL
                .iter()
                .all(|form| form.tag_folder() == "block" || form.tag_folder() == "item")
        );
        assert_eq!(Form::Ore.tag_folder(), "block");
        assert_eq!(Form::Ingot.tag_folder(), "item");
    }
}
