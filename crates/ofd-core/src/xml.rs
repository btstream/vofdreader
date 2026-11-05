use crate::error::{OfdError, Result};

pub fn parse_xml<'a>(bytes: &'a [u8], file: &str) -> Result<roxmltree::Document<'a>> {
    let text = std::str::from_utf8(bytes).map_err(|_| OfdError::NonUtf8(file.to_string()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    roxmltree::Document::parse(text).map_err(|source| OfdError::Xml {
        file: file.to_string(),
        source,
    })
}

pub fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

pub fn children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> + 'a {
    node.children()
        .filter(move |child| child.is_element() && child.tag_name().name() == name)
}

pub fn child_text<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<&'a str> {
    child(node, name)
        .and_then(|child| child.text())
        .map(str::trim)
}

pub fn required_attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Result<&'a str> {
    node.attribute(name)
        .ok_or_else(|| OfdError::MissingAttribute(name.to_string()))
}

pub fn parse_f64(value: &str, what: &str) -> Result<f64> {
    value
        .trim()
        .parse::<f64>()
        .map_err(|_| OfdError::InvalidValue {
            what: what.to_string(),
            value: value.to_string(),
        })
}
