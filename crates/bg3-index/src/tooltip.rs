use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::localization::valid_handle;
use crate::package::read_package_entry;
use crate::xml::attributes;

const TOOLTIP_GLOSSARY_ENTRY: &str = "Public/Game/GUI/Library/Tooltips.xaml";
const MAX_TOOLTIP_GLOSSARY_SIZE: usize = 4 * 1024 * 1024;

/// Static localization handles for one game tooltip key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TooltipText {
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Read-only static tooltip keys extracted from the game UI glossary.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TooltipCatalog {
    entries: BTreeMap<String, TooltipText>,
}

impl TooltipCatalog {
    /// Returns the static text handles for one exact tooltip key.
    pub fn get(&self, key: &str) -> Option<&TooltipText> {
        self.entries.get(key)
    }

    /// Returns the number of static tooltip keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Tests whether no static tooltip keys are available.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Reads the canonical static tooltip glossary when `Game.pak` exists.
pub fn read_base_tooltip_catalog(game_data: &Path) -> Result<Option<TooltipCatalog>, Error> {
    let path = base_tooltip_package_path(game_data);
    if !path.is_file() {
        return Ok(None);
    }
    let xaml = read_package_entry(&path, TOOLTIP_GLOSSARY_ENTRY, MAX_TOOLTIP_GLOSSARY_SIZE)?;
    parse_tooltip_catalog(&xaml).map(Some)
}

/// Extracts only entries whose XAML tooltip has static localization handles.
pub fn parse_tooltip_catalog(xaml: &[u8]) -> Result<TooltipCatalog, Error> {
    let mut reader = Reader::from_reader(xaml);
    reader.config_mut().trim_text(false);
    let mut current: Option<(String, Option<String>, Option<String>)> = None;
    let mut entries = BTreeMap::new();

    loop {
        match reader.read_event()? {
            Event::Start(event) if event.local_name().as_ref() == b"Trigger" => {
                current = tooltip_key(&event)?.map(|key| (key, None, None));
            }
            Event::Start(event) | Event::Empty(event)
                if current.is_some() && event.local_name().as_ref() == b"LSTooltip" =>
            {
                let values = attributes(&event)?;
                let (_, title, description) = current.as_mut().expect("current was checked");
                *title = values
                    .get("ls:AttachedProperties.InheritedTag")
                    .or_else(|| values.get("Tag"))
                    .filter(|value| valid_handle(value))
                    .cloned();
                *description = values
                    .get("Content")
                    .filter(|value| valid_handle(value))
                    .cloned();
            }
            Event::End(event) if event.local_name().as_ref() == b"Trigger" => {
                if let Some((key, title, description)) = current.take()
                    && (title.is_some() || description.is_some())
                {
                    entries.insert(key, TooltipText { title, description });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(TooltipCatalog { entries })
}

fn tooltip_key(event: &BytesStart<'_>) -> Result<Option<String>, Error> {
    let values = attributes(event)?;
    Ok(
        (values.get("Property").map(String::as_str) == Some("TagTooltip"))
            .then(|| values.get("Value").cloned())
            .flatten(),
    )
}

/// Returns the package path for cache keys and file watches.
pub fn base_tooltip_package_path(game_data: &Path) -> PathBuf {
    game_data.join("Game.pak")
}
