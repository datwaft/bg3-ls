use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::xml::attributes;

/// Metadata for one field in a Toolkit schema definition.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SchemaField {
    pub name: String,
    pub field_type: Option<String>,
    pub display_name: Option<String>,
    pub export_name: Option<String>,
    pub description: Option<String>,
    pub enumeration_type_name: Option<String>,
    pub min_value: Option<String>,
    pub max_value: Option<String>,
    pub delimiter: Option<String>,
    pub autocomplete_type: Option<String>,
    pub compilation_type: Option<String>,
    pub object_type: Option<String>,
    pub is_internal: bool,
    pub auto_generated: bool,
}

/// Metadata for one Stats or UUID-object schema.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SchemaDefinition {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub export_name: Option<String>,
    pub export_type: Option<String>,
    pub object_type: Option<String>,
    pub fields: BTreeMap<String, SchemaField>,
}

impl SchemaDefinition {
    /// Finds one field by its Toolkit name or its legacy export name.
    pub fn field(&self, name: &str) -> Option<&SchemaField> {
        self.fields.get(name).or_else(|| {
            self.fields
                .values()
                .find(|field| field.export_name.as_deref() == Some(name))
        })
    }
}

impl SchemaField {
    /// Returns the field name that legacy Stats files store in `data` clauses.
    pub fn legacy_name(&self) -> &str {
        self.export_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.name)
    }
}

/// All schema and enumeration data needed to interpret BG3 sources.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SchemaCatalog {
    pub by_id: BTreeMap<String, SchemaDefinition>,
    pub enumerations: BTreeMap<String, Vec<String>>,
}

impl SchemaCatalog {
    /// Parses and merges one Toolkit schema catalog.
    pub fn merge_definitions(&mut self, source: &str) -> Result<(), Error> {
        let mut reader = Reader::from_str(source);
        reader.config_mut().trim_text(true);
        let mut current: Option<SchemaDefinition> = None;

        loop {
            match reader.read_event()? {
                Event::Start(event) if event.name().as_ref() == b"stat_object_definition" => {
                    current = Some(definition_from_attributes(&attributes(&event)?)?);
                }
                Event::Empty(event) if event.name().as_ref() == b"field_definition" => {
                    let field = field_from_attributes(&attributes(&event)?)?;
                    let definition = current.as_mut().ok_or_else(|| {
                        Error::Schema("field_definition is outside a schema definition".into())
                    })?;
                    definition.fields.insert(field.name.clone(), field);
                }
                Event::End(event) if event.name().as_ref() == b"stat_object_definition" => {
                    let definition = current.take().ok_or_else(|| {
                        Error::Schema("schema definition ended before it started".into())
                    })?;
                    self.by_id.insert(definition.id.clone(), definition);
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// Parses and merges one Toolkit enumeration catalog without duplicates.
    pub fn merge_enumerations(&mut self, source: &str) -> Result<(), Error> {
        let mut reader = Reader::from_str(source);
        reader.config_mut().trim_text(true);
        let mut current: Option<String> = None;
        let mut values: BTreeMap<String, BTreeSet<String>> = self
            .enumerations
            .iter()
            .map(|(name, values)| (name.clone(), values.iter().cloned().collect()))
            .collect();

        loop {
            match reader.read_event()? {
                Event::Start(event) if event.name().as_ref() == b"enumeration" => {
                    current = attributes(&event)?.get("name").cloned();
                }
                Event::Empty(event) if event.name().as_ref() == b"item" => {
                    if let (Some(name), Some(value)) =
                        (current.as_ref(), attributes(&event)?.get("value"))
                    {
                        values
                            .entry(name.clone())
                            .or_default()
                            .insert(value.clone());
                    }
                }
                Event::End(event) if event.name().as_ref() == b"enumeration" => current = None,
                Event::Eof => break,
                _ => {}
            }
        }

        self.enumerations = values
            .into_iter()
            .map(|(name, values)| (name, values.into_iter().collect()))
            .collect();
        Ok(())
    }

    /// Infers all valid schemas for one legacy Stats entry.
    pub fn infer<'a>(&'a self, path: &Path, entry_kind: Option<&str>) -> Vec<&'a SchemaDefinition> {
        self.infer_legacy(path, entry_kind, &BTreeMap::new())
    }

    /// Infers legacy schemas from the path, entry kind, and type discriminator.
    pub fn infer_legacy<'a>(
        &'a self,
        path: &Path,
        entry_kind: Option<&str>,
        fields: &BTreeMap<String, String>,
    ) -> Vec<&'a SchemaDefinition> {
        let stem = path.file_stem().and_then(|value| value.to_str());
        let candidates: Vec<_> = self
            .by_id
            .values()
            .filter(|definition| {
                entry_kind.is_none_or(|kind| {
                    definition.export_type.as_deref() == Some(kind)
                        || definition.category.as_deref() == Some(kind)
                        || definition.name == kind
                })
            })
            .collect();
        let path_matches = stem.map_or_else(Vec::new, |stem| {
            candidates
                .iter()
                .copied()
                .filter(|definition| definition.name.eq_ignore_ascii_case(stem))
                .collect()
        });
        let discriminator_matches =
            discriminator(entry_kind, fields).map_or_else(Vec::new, |(_, value, prefix)| {
                let schema_name = format!("{prefix}{value}");
                candidates
                    .iter()
                    .copied()
                    .filter(|definition| definition.name.eq_ignore_ascii_case(&schema_name))
                    .collect()
            });

        match (path_matches.is_empty(), discriminator_matches.is_empty()) {
            (true, true) => candidates,
            (false, true) => path_matches,
            (true, false) => discriminator_matches,
            (false, false) => {
                let intersection: Vec<_> = path_matches
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        discriminator_matches
                            .iter()
                            .any(|other| other.id == candidate.id)
                    })
                    .collect();
                if intersection.is_empty() {
                    let mut union = path_matches;
                    union.extend(discriminator_matches);
                    union.sort_by(|left, right| left.id.cmp(&right.id));
                    union.dedup_by(|left, right| left.id == right.id);
                    union
                } else {
                    intersection
                }
            }
        }
    }

    /// Returns a verified conflict for one legacy schema discriminator.
    pub fn discriminator_error(
        &self,
        path: &Path,
        entry_kind: Option<&str>,
        fields: &BTreeMap<String, String>,
    ) -> Option<(&'static str, String)> {
        let (field, value, prefix) = discriminator(entry_kind, fields)?;
        let candidates: Vec<_> = self
            .by_id
            .values()
            .filter(|definition| {
                entry_kind.is_none_or(|kind| {
                    definition.export_type.as_deref() == Some(kind)
                        || definition.category.as_deref() == Some(kind)
                        || definition.name == kind
                })
            })
            .collect();
        let selected_name = format!("{prefix}{value}");
        let selected: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|definition| definition.name.eq_ignore_ascii_case(&selected_name))
            .collect();
        if selected.is_empty() {
            return Some((
                field,
                format!("`{value}` does not identify a known `{field}` schema."),
            ));
        }
        let stem = path.file_stem().and_then(|value| value.to_str())?;
        let path_schema = candidates
            .iter()
            .copied()
            .find(|definition| definition.name.eq_ignore_ascii_case(stem));
        if let Some(path_schema) = path_schema
            && selected
                .iter()
                .all(|definition| definition.id != path_schema.id)
        {
            return Some((
                field,
                format!(
                    "`{field}` selects `{selected_name}`, but the source path selects `{}`.",
                    path_schema.name
                ),
            ));
        }
        None
    }

    /// Returns a stable digest used to invalidate schema-dependent file caches.
    pub fn digest(&self) -> Result<blake3::Hash, Error> {
        Ok(blake3::hash(&postcard::to_stdvec(self)?))
    }
}

/// Tests whether one legacy field selects a Stats schema subtype.
pub fn is_schema_discriminator(entry_kind: &str, field: &str) -> bool {
    matches!(
        (entry_kind, field),
        ("StatusData", "StatusType") | ("SpellData", "SpellType")
    )
}

/// Extracts one applicable legacy type discriminator and its schema prefix.
fn discriminator<'a>(
    entry_kind: Option<&str>,
    fields: &'a BTreeMap<String, String>,
) -> Option<(&'static str, &'a str, &'static str)> {
    match entry_kind {
        Some("StatusData") => fields
            .get("StatusType")
            .map(|value| ("StatusType", value.as_str(), "Status_")),
        Some("SpellData") => fields
            .get("SpellType")
            .map(|value| ("SpellType", value.as_str(), "Spell_")),
        _ => None,
    }
}

/// Converts XML attributes to one schema definition.
fn definition_from_attributes(
    values: &BTreeMap<String, String>,
) -> Result<SchemaDefinition, Error> {
    Ok(SchemaDefinition {
        id: required(values, "id")?,
        name: required(values, "name")?,
        category: values.get("category").cloned(),
        export_name: values.get("export_name").cloned(),
        export_type: values.get("export_type").cloned(),
        object_type: values.get("object_type").cloned(),
        fields: BTreeMap::new(),
    })
}

/// Converts XML attributes to one schema field.
fn field_from_attributes(values: &BTreeMap<String, String>) -> Result<SchemaField, Error> {
    Ok(SchemaField {
        name: required(values, "name")?,
        field_type: values.get("type").cloned(),
        display_name: values.get("display_name").cloned(),
        export_name: values.get("export_name").cloned(),
        description: values.get("description").cloned(),
        enumeration_type_name: values.get("enumeration_type_name").cloned(),
        min_value: values.get("min_value").cloned(),
        max_value: values.get("max_value").cloned(),
        delimiter: values.get("delimiter").cloned(),
        autocomplete_type: values.get("autocomplete_type").cloned(),
        compilation_type: values.get("compilation_type").cloned(),
        object_type: values.get("object_type").cloned(),
        is_internal: bool_attribute(values.get("is_internal")),
        auto_generated: bool_attribute(values.get("auto_generated")),
    })
}

/// Reads one required XML attribute.
fn required(values: &BTreeMap<String, String>, name: &str) -> Result<String, Error> {
    values
        .get(name)
        .cloned()
        .ok_or_else(|| Error::Schema(format!("required schema attribute `{name}` is missing")))
}

/// Interprets the boolean spellings used by Toolkit schema files.
fn bool_attribute(value: Option<&String>) -> bool {
    value.is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}
