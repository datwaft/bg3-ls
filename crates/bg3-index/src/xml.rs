use std::collections::BTreeMap;

use quick_xml::XmlVersion;
use quick_xml::events::BytesStart;

use crate::Error;
use crate::domain::{LineMap, TextRange};

/// Decodes all attributes on one XML start or empty event.
pub(crate) fn attributes(event: &BytesStart<'_>) -> Result<BTreeMap<String, String>, Error> {
    let mut values = BTreeMap::new();
    for attribute in event.attributes() {
        let attribute = attribute?;
        let name = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)?
            .into_owned();
        values.insert(name, value);
    }
    Ok(values)
}

/// Finds the range of an attribute value inside an XML event.
pub(crate) fn attribute_range(
    source: &str,
    lines: &LineMap,
    event_start: usize,
    event_end: usize,
    name: &str,
) -> Option<TextRange> {
    let event = source.get(event_start..event_end)?;
    for quote in ['"', '\''] {
        let marker = format!("{name}={quote}");
        if let Some(relative) = event.find(&marker) {
            let start = event_start + relative + marker.len();
            let length = source.get(start..)?.find(quote)?;
            return Some(lines.range(start, start + length));
        }
    }
    None
}
