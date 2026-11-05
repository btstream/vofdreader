use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::content::parse_page_content;
use crate::error::{OfdError, Result};
use crate::loc::{join_dir, resolve_loc};
use crate::model::{
    AnnotInfo, AnnotationPage, Color, CompositeUnit, DocInfo, DocumentMeta, DrawParam,
    FontResource, ImageResource, OfdInfo, PageRef, PageSummary, ResScope, ResourceSet, SizeMm,
    TemplateRef, parse_physical_box,
};
use crate::package::Package;
use crate::render::Item;
use crate::xml::{child, child_text, children, parse_xml, required_attr};

/// An opened OFD document.
///
/// Metadata, resources and annotations are parsed eagerly (they are small);
/// page content is parsed lazily per page.
pub struct OfdDocument {
    pub package: Package,
    pub entry: String,
    pub ofd: OfdInfo,
    pub meta: DocumentMeta,
    pub document_file: String,
    pub resources: ResourceSet,
    pub annotations: Vec<AnnotationPage>,
    pub warnings: Vec<String>,
    signature_seals: OnceLock<Vec<SealStamp>>,
}

#[derive(Debug, Clone)]
pub(crate) struct SealStamp {
    pub page_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub clip: Option<(f64, f64, f64, f64)>,
    pub mime: &'static str,
    pub data: Arc<Vec<u8>>,
}

impl OfdDocument {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let package = Package::open(path)?;
        Self::from_package(package, true)
    }

    // Nested seal appearances must not recursively load signature appearances.
    fn from_seal_bytes(data: Vec<u8>) -> Result<Self> {
        Self::from_package(Package::from_bytes(data)?, false)
    }

    fn from_package(package: Package, allow_signatures: bool) -> Result<Self> {
        let entry = package.root_entry()?;

        let ofd_bytes = package.read(&entry)?;
        let ofd_doc = parse_xml(&ofd_bytes, &entry)?;
        let ofd_root = ofd_doc.root_element();

        if ofd_root.tag_name().name() != "OFD" {
            return Err(OfdError::MissingElement("OFD 根节点".to_string()));
        }

        let version = ofd_root.attribute("Version").unwrap_or("1.0").to_string();
        let doc_type = ofd_root.attribute("DocType").unwrap_or("OFD").to_string();

        let body = child(ofd_root, "DocBody")
            .ok_or_else(|| OfdError::MissingElement("DocBody".to_string()))?;
        let info_node = child(body, "DocInfo");

        let info = DocInfo {
            doc_id: info_node
                .and_then(|node| child_text(node, "DocID"))
                .map(str::to_string),
            title: info_node
                .and_then(|node| child_text(node, "Title"))
                .map(str::to_string),
            author: info_node
                .and_then(|node| child_text(node, "Author"))
                .map(str::to_string),
            creator: info_node
                .and_then(|node| child_text(node, "Creator"))
                .map(str::to_string),
            creation_date: info_node
                .and_then(|node| child_text(node, "CreationDate"))
                .map(str::to_string),
            mod_date: info_node
                .and_then(|node| child_text(node, "ModDate"))
                .map(str::to_string),
        };

        let doc_root = child_text(body, "DocRoot")
            .ok_or_else(|| OfdError::MissingElement("DocRoot".to_string()))?;
        let document_file = resolve_loc(&entry, doc_root)?;
        let signatures = allow_signatures
            .then(|| child_text(body, "Signatures").map(str::to_string))
            .flatten();

        let ofd = OfdInfo {
            version,
            doc_type,
            info,
            doc_root: document_file.clone(),
            signatures,
        };

        let document_bytes = package.read(&document_file)?;
        let document_doc = parse_xml(&document_bytes, &document_file)?;
        let document_root = document_doc.root_element();

        if document_root.tag_name().name() != "Document" {
            return Err(OfdError::MissingElement("Document 根节点".to_string()));
        }

        let common = child(document_root, "CommonData");
        let public_res = common
            .and_then(|node| child_text(node, "PublicRes"))
            .map(|loc| resolve_loc(&document_file, loc))
            .transpose()?;
        let document_res = common
            .and_then(|node| child_text(node, "DocumentRes"))
            .map(|loc| resolve_loc(&document_file, loc))
            .transpose()?;

        let mut meta = DocumentMeta {
            public_res: public_res.clone(),
            document_res: document_res.clone(),
            ..Default::default()
        };

        meta.page_area = common
            .and_then(|node| child(node, "PageArea"))
            .and_then(|node| child_text(node, "PhysicalBox"))
            .and_then(parse_physical_box);

        // TemplatePage can be a direct child of CommonData (seen in real
        // invoices) or live under Pages; scan descendants to be tolerant.
        for node in document_root
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "TemplatePage")
        {
            if let Some(base_loc) = node.attribute("BaseLoc") {
                meta.template_pages.push(TemplateRef {
                    id: node.attribute("ID").unwrap_or("").to_string(),
                    base_loc: base_loc.to_string(),
                });
            }
        }

        let pages_node = child(document_root, "Pages")
            .ok_or_else(|| OfdError::MissingElement("Pages".to_string()))?;
        for (index, page) in children(pages_node, "Page").enumerate() {
            meta.pages.push(PageRef {
                id: page
                    .attribute("ID")
                    .map(str::to_string)
                    .unwrap_or_else(|| index.to_string()),
                base_loc: required_attr(page, "BaseLoc")?.to_string(),
            });
        }

        if meta.pages.is_empty() {
            return Err(OfdError::MissingElement("Page".to_string()));
        }

        meta.annotations = child_text(document_root, "Annotations")
            .map(|loc| resolve_loc(&document_file, loc))
            .transpose()?;

        let mut warnings = Vec::new();
        let mut resources = ResourceSet::default();

        if let Some(public_loc) = public_res {
            match load_resources(&package, &public_loc, ResScope::Public) {
                Ok(mut loaded) => {
                    resources.fonts.append(&mut loaded.fonts);
                    resources.images.append(&mut loaded.images);
                    resources
                        .composite_units
                        .append(&mut loaded.composite_units);
                    resources.color_spaces += loaded.color_spaces;
                    resources.draw_params.append(&mut loaded.draw_params);
                    warnings.append(&mut loaded.warnings);
                }
                Err(error) => warnings.push(format!("公有资源读取失败：{error}")),
            }
        }

        if let Some(document_loc) = document_res {
            match load_resources(&package, &document_loc, ResScope::Document) {
                Ok(mut loaded) => {
                    resources.fonts.append(&mut loaded.fonts);
                    resources.images.append(&mut loaded.images);
                    resources
                        .composite_units
                        .append(&mut loaded.composite_units);
                    resources.color_spaces += loaded.color_spaces;
                    resources.draw_params.append(&mut loaded.draw_params);
                    warnings.append(&mut loaded.warnings);
                }
                Err(error) => warnings.push(format!("文档资源读取失败：{error}")),
            }
        }

        let annotations = match meta.annotations.as_deref() {
            Some(loc) => load_annotations(&package, loc, &mut warnings),
            None => Vec::new(),
        };

        Ok(Self {
            package,
            entry,
            ofd,
            meta,
            document_file,
            resources,
            annotations,
            warnings,
            signature_seals: OnceLock::new(),
        })
    }

    pub(crate) fn signature_seals(&self) -> &[SealStamp] {
        self.signature_seals
            .get_or_init(|| load_signature_seals(self))
            .as_slice()
    }

    pub fn page_count(&self) -> usize {
        self.meta.pages.len()
    }

    pub fn page(&self, index: usize) -> Result<&PageRef> {
        self.meta
            .pages
            .get(index)
            .ok_or_else(|| OfdError::InvalidValue {
                what: "page index".to_string(),
                value: index.to_string(),
            })
    }

    pub fn page_content_loc(&self, index: usize) -> Result<String> {
        let page = self.page(index)?;
        resolve_loc(&self.document_file, &page.base_loc)
    }

    pub fn page_summary(&self, index: usize) -> Result<PageSummary> {
        let page = self.page(index)?;
        let loc = resolve_loc(&self.document_file, &page.base_loc)?;
        let bytes = self.package.read_shared(&loc)?;
        let mut summary = parse_page_content(&loc, &bytes, index, &page.id, self.meta.page_area)?;

        let declared: BTreeSet<&str> = self.resources.font_ids().collect();
        for font in &summary.used_fonts {
            if !declared.contains(font.as_str()) {
                summary
                    .warnings
                    .push(format!("字体资源 {font} 未声明，将回退到系统字体"));
            }
        }

        Ok(summary)
    }

    pub fn page_size(&self, index: usize) -> Result<Option<SizeMm>> {
        self.page_size_only(index)
    }

    /// Reads only the page area, without counting every object in the page.
    /// The desktop app uses it to discover sizes in the background.
    pub fn page_size_only(&self, index: usize) -> Result<Option<SizeMm>> {
        let page = self.page(index)?;
        let loc = resolve_loc(&self.document_file, &page.base_loc)?;
        let bytes = self.package.read_shared(&loc)?;
        let document = parse_xml(&bytes, &loc)?;
        let root = document.root_element();
        let size = root
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "Area")
            .and_then(|area| child_text(area, "PhysicalBox"))
            .and_then(parse_physical_box)
            .or(self.meta.page_area);
        Ok(size)
    }

    pub fn page_text(&self, index: usize) -> Result<Vec<String>> {
        Ok(self.page_summary(index)?.text)
    }

    /// Text of the rendered page, including background templates. Search uses
    /// this so invoice titles and table labels declared in the template are
    /// findable, not just the page's own content stream.
    pub fn page_display_text(&self, index: usize) -> Result<Vec<String>> {
        fn collect(items: &[Item], out: &mut Vec<String>) {
            for item in items {
                match item {
                    Item::Text { chars, .. } => {
                        let text: String = chars.iter().map(|char| char.ch).collect();
                        if !text.trim().is_empty() {
                            out.push(text);
                        }
                    }
                    Item::Group { items, .. } => collect(items, out),
                    _ => {}
                }
            }
        }

        let list = self.display_list(index)?;
        let mut out = Vec::new();
        collect(&list.items, &mut out);
        Ok(out)
    }

    pub fn annotations_for_page(&self, page_id: &str) -> Option<&AnnotationPage> {
        self.annotations.iter().find(|page| page.page_id == page_id)
    }

    /// Detect signature material without verifying it yet. Verification arrives
    /// with the signature milestone; for now the UI can honestly say
    /// "contains signature (unverified)".
    pub fn has_signature_material(&self) -> bool {
        self.ofd.signatures.is_some()
            || self.package.entries().iter().any(|entry| {
                entry.ends_with("/Signatures.xml")
                    || entry.ends_with("Signatures.xml")
                    || entry.contains("/Signs/")
            })
    }

    pub fn check(&self) -> Vec<String> {
        let mut issues = self.warnings.clone();

        for (index, _) in self.meta.pages.iter().enumerate() {
            match self.page_summary(index) {
                Ok(summary) => {
                    for warning in summary.warnings {
                        issues.push(format!("第 {} 页：{warning}", index + 1));
                    }
                }
                Err(error) => issues.push(format!("第 {} 页解析失败：{error}", index + 1)),
            }
        }

        for font in &self.resources.fonts {
            if !font.embedded() {
                issues.push(format!(
                    "字体 {}（{}）未内嵌，需要系统字体或回退",
                    font.id, font.name
                ));
            }
        }

        for image in &self.resources.images {
            if image.file.is_none() {
                issues.push(format!("图像资源 {} 缺少 MediaFile", image.id));
            }
        }

        issues
    }
}

fn parent_dir(file: &str) -> &str {
    file.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

fn load_resources(package: &Package, file: &str, scope: ResScope) -> Result<ResourceSet> {
    let bytes = package.read(file)?;
    let document = parse_xml(&bytes, file)?;
    let root = document.root_element();
    let mut set = ResourceSet::default();

    if root.tag_name().name() != "Res" {
        return Err(OfdError::MissingElement("Res 根节点".to_string()));
    }

    let base_dir = match root.attribute("BaseLoc") {
        Some(base) if !base.trim().is_empty() => join_dir(parent_dir(file), base)?,
        _ => parent_dir(file).to_string(),
    };

    if let Some(fonts) = child(root, "Fonts") {
        for node in children(fonts, "Font") {
            let id = node.attribute("ID").unwrap_or("").to_string();
            let name = node
                .attribute("FontName")
                .or_else(|| node.attribute("FamilyName"))
                .unwrap_or("")
                .to_string();
            let family = node
                .attribute("FamilyName")
                .unwrap_or(name.as_str())
                .to_string();

            let mut font_file = child_text(node, "FontFile")
                .map(|loc| join_dir(&base_dir, loc))
                .transpose()?;
            if let Some(path) = &mut font_file {
                match package.resolve(path) {
                    Ok(resolved) => *path = resolved,
                    Err(_) => {
                        set.warnings
                            .push(format!("字体资源 {id} 的 FontFile 不存在：{path}"));
                        font_file = None;
                    }
                }
            }

            set.fonts.push(FontResource {
                id,
                scope,
                name,
                family,
                file: font_file,
                bold: node
                    .attribute("Bold")
                    .is_some_and(|v| v == "true" || v == "1"),
                italic: node
                    .attribute("Italic")
                    .is_some_and(|v| v == "true" || v == "1"),
            });
        }
    }

    if let Some(media) = child(root, "MultiMedias") {
        for node in children(media, "MultiMedia") {
            let id = node.attribute("ID").unwrap_or("").to_string();
            let format = node.attribute("Format").map(str::to_string);
            let mut image_file = child_text(node, "MediaFile")
                .map(|loc| join_dir(&base_dir, loc))
                .transpose()?;
            if let Some(path) = &mut image_file {
                match package.resolve(path) {
                    Ok(resolved) => *path = resolved,
                    Err(_) => {
                        set.warnings
                            .push(format!("图像资源 {id} 的文件不存在：{path}"));
                        image_file = None;
                    }
                }
            }
            set.images.push(ImageResource {
                id,
                scope,
                format,
                file: image_file,
            });
        }
    }

    if let Some(groups) = child(root, "CompositeGraphicUnits") {
        for node in children(groups, "CompositeGraphicUnit") {
            let id = node.attribute("ID").unwrap_or("").to_string();
            let width = node
                .attribute("Width")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);
            let height = node
                .attribute("Height")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);

            let content = if let Some(base) = node.attribute("BaseLoc") {
                let base = base.trim();
                if base.is_empty() {
                    None
                } else {
                    let path = join_dir(&base_dir, base).and_then(|path| {
                        package.resolve(&path).map_err(|_| OfdError::InvalidValue {
                            what: "CompositeGraphicUnit BaseLoc".to_string(),
                            value: path,
                        })
                    });
                    match path.and_then(|path| package.read(&path)) {
                        Ok(bytes) => String::from_utf8(bytes).ok(),
                        Err(_) => {
                            set.warnings
                                .push(format!("复合图形资源 {id} 的 BaseLoc 读取失败：{base}"));
                            None
                        }
                    }
                }
            } else {
                let range = node.range();
                std::str::from_utf8(&bytes[range]).ok().map(|fragment| {
                    format!(
                        r#"<ofd:Res xmlns:ofd="http://www.ofdspec.org/2016" xmlns="http://www.ofdspec.org/2016">{fragment}</ofd:Res>"#
                    )
                })
            };

            if let Some(content) = content {
                set.composite_units.push(CompositeUnit {
                    id,
                    scope,
                    width,
                    height,
                    content,
                });
            }
        }
    }

    set.color_spaces = child(root, "ColorSpaces")
        .map(|node| children(node, "ColorSpace").count())
        .unwrap_or(0);

    if let Some(params) = child(root, "DrawParams") {
        for node in children(params, "DrawParam") {
            let mut param = DrawParam {
                id: node.attribute("ID").unwrap_or("").to_string(),
                scope,
                relative: node.attribute("Relative").map(str::to_string),
                line_width: node
                    .attribute("LineWidth")
                    .and_then(|value| value.parse().ok()),
                cap: node.attribute("Cap").map(str::to_string),
                join: node.attribute("Join").map(str::to_string),
                dash_pattern: node
                    .attribute("DashPattern")
                    .map(|value| {
                        value
                            .split_whitespace()
                            .filter_map(|part| part.parse().ok())
                            .collect()
                    })
                    .unwrap_or_default(),
                ..Default::default()
            };

            if let Some(color) = child(node, "StrokeColor") {
                param.stroke_color = read_color(color);
            }
            if let Some(color) = child(node, "FillColor") {
                param.fill_color = read_color(color);
            }
            set.draw_params.push(param);
        }
    }

    Ok(set)
}

fn load_annotations(
    package: &Package,
    file: &str,
    warnings: &mut Vec<String>,
) -> Vec<AnnotationPage> {
    let bytes = match package.read(file) {
        Ok(bytes) => bytes,
        Err(error) => {
            warnings.push(format!("注释文件读取失败：{error}"));
            return Vec::new();
        }
    };
    let document = match parse_xml(&bytes, file) {
        Ok(document) => document,
        Err(error) => {
            warnings.push(format!("注释文件解析失败：{error}"));
            return Vec::new();
        }
    };

    let root = document.root_element();
    let mut result = Vec::new();

    for page in children(root, "Page") {
        let page_id = page.attribute("PageID").unwrap_or("").to_string();
        let Some(file_loc) = child_text(page, "FileLoc") else {
            warnings.push(format!("注释页 {page_id} 缺少 FileLoc"));
            continue;
        };
        let file_loc = match resolve_loc(file, file_loc) {
            Ok(loc) => loc,
            Err(error) => {
                warnings.push(format!("注释页 {page_id} 路径非法：{error}"));
                continue;
            }
        };

        let annot_bytes = match package.read(&file_loc) {
            Ok(bytes) => bytes,
            Err(error) => {
                warnings.push(format!("注释页 {page_id} 读取失败：{error}"));
                continue;
            }
        };
        let annot_doc = match parse_xml(&annot_bytes, &file_loc) {
            Ok(doc) => doc,
            Err(error) => {
                warnings.push(format!("注释页 {page_id} 解析失败：{error}"));
                continue;
            }
        };

        let mut annots = Vec::new();
        for annot in children(annot_doc.root_element(), "Annot") {
            let kind = annot.attribute("Type").unwrap_or("Unknown").to_string();
            let subtype = annot.attribute("Subtype").map(str::to_string);
            let boundary = child(annot, "Appearance")
                .and_then(|appearance| appearance.attribute("Boundary"))
                .map(str::to_string);

            let mut text = Vec::new();
            let mut image_resources = Vec::new();
            let mut path_objects = 0;
            if let Some(appearance) = child(annot, "Appearance") {
                walk_appearance(
                    appearance,
                    &mut text,
                    &mut image_resources,
                    &mut path_objects,
                );
            }

            annots.push(AnnotInfo {
                id: annot.attribute("ID").unwrap_or("").to_string(),
                kind,
                subtype,
                boundary,
                text,
                image_resources,
                path_objects,
            });
        }

        result.push(AnnotationPage {
            page_id,
            file: file_loc,
            annots,
        });
    }

    result
}

fn read_color(node: roxmltree::Node<'_, '_>) -> Option<Color> {
    let value = node.attribute("Value")?;
    let alpha = node
        .attribute("Alpha")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(255);
    Color::parse(value, alpha)
}

fn walk_appearance(
    node: roxmltree::Node<'_, '_>,
    text: &mut Vec<String>,
    images: &mut Vec<String>,
    paths: &mut usize,
) {
    for node in node.descendants().filter(|node| node.is_element()) {
        match node.tag_name().name() {
            "TextObject" => {
                if let Some(code) = child(node, "TextCode")
                    && let Some(value) = code.text()
                {
                    let value = value.trim();
                    if !value.is_empty() {
                        text.push(value.to_string());
                    }
                }
            }
            "ImageObject" => {
                if let Some(resource) = node.attribute("ResourceID") {
                    images.push(resource.to_string());
                }
            }
            "PathObject" => *paths += 1,
            _ => {}
        }
    }
}

fn load_signature_seals(doc: &OfdDocument) -> Vec<SealStamp> {
    let Some(signatures_loc) = &doc.ofd.signatures else {
        return Vec::new();
    };
    let Ok(signatures_file) = resolve_loc(&doc.entry, signatures_loc) else {
        return Vec::new();
    };
    let Ok(bytes) = doc.package.read_shared(&signatures_file) else {
        return Vec::new();
    };
    let Ok(document) = parse_xml(&bytes, &signatures_file) else {
        return Vec::new();
    };

    let mut stamps = Vec::new();
    for signature in children(document.root_element(), "Signature") {
        let Some(base_loc) = signature.attribute("BaseLoc") else {
            continue;
        };
        let Ok(signature_file) = resolve_loc(&signatures_file, base_loc) else {
            continue;
        };
        let Ok(signature_bytes) = doc.package.read_shared(&signature_file) else {
            continue;
        };
        let Ok(signature_doc) = parse_xml(&signature_bytes, &signature_file) else {
            continue;
        };
        let root = signature_doc.root_element();
        let seal_loc = root
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "Seal")
            .and_then(|seal| child_text(seal, "BaseLoc"));
        // Some producers only carry the SES seal inside SignedValue.
        let image = [seal_loc, child_text(root, "SignedValue")]
            .into_iter()
            .flatten()
            .find_map(|loc| {
                let file = resolve_loc(&signature_file, loc).ok()?;
                let bytes = doc.package.read(&file).ok()?;
                decode_seal_appearance(&bytes)
            });
        let Some((image, mime)) = image else {
            continue;
        };
        let image = Arc::new(image);

        for annot in root
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "StampAnnot")
        {
            let Some(page_id) = annot.attribute("PageRef") else {
                continue;
            };
            let Some((x, y, width, height)) = annot
                .attribute("Boundary")
                .and_then(crate::model::parse_box)
            else {
                continue;
            };
            let clip = annot.attribute("Clip").and_then(crate::model::parse_box);
            stamps.push(SealStamp {
                page_id: page_id.to_string(),
                x,
                y,
                width,
                height,
                clip,
                mime,
                data: image.clone(),
            });
        }
    }
    stamps
}

fn decode_seal_appearance(data: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    if let Some((kind, picture)) = seal_picture(data, 0) {
        if kind.eq_ignore_ascii_case(b"ofd") {
            // No temporary extraction, external paths, or nested signature
            // recursion. Package applies the usual ZIP entry/size limits.
            if picture.len() > 16 * 1024 * 1024 {
                return None;
            }
            let doc = OfdDocument::from_seal_bytes(picture.to_vec()).ok()?;
            let svg = crate::render::PageRenderer::new(&doc)
                .render_seal_svg(&doc)
                .ok()?;
            return Some((svg.into_bytes(), "image/svg+xml"));
        }
        if let Some(image) = extract_embedded_image(picture) {
            return Some(image);
        }
    }
    extract_embedded_image(data)
}

/// Read one definite-length DER field, rejecting truncation and arithmetic
/// overflow. This only decodes appearance metadata; it does not verify a seal.
fn der_field(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = data.split_first()?;
    let (&length, rest) = rest.split_first()?;
    let (length, body) = if length & 0x80 == 0 {
        (length as usize, rest)
    } else {
        let count = (length & 0x7f) as usize;
        if count == 0 || count > std::mem::size_of::<usize>() || count > rest.len() {
            return None;
        }
        let mut length = 0usize;
        for byte in &rest[..count] {
            length = length.checked_mul(256)?.checked_add(*byte as usize)?;
        }
        (length, &rest[count..])
    };
    if length > body.len() {
        return None;
    }
    Some((tag, &body[..length], &body[length..]))
}

/// SES picture info is a SEQUENCE { type IA5String, data OCTET STRING,
/// width INTEGER, height INTEGER }. Locate the typed field, not a coincidental
/// ZIP magic inside a certificate or some unrelated signed payload.
fn seal_picture(mut data: &[u8], depth: usize) -> Option<(&[u8], &[u8])> {
    if depth > 16 {
        return None;
    }
    while !data.is_empty() {
        let (tag, value, rest) = der_field(data)?;
        if tag == 0x30 {
            if let Some((0x16, kind, fields)) = der_field(value)
                && [
                    b"ofd".as_slice(),
                    b"png".as_slice(),
                    b"jpg".as_slice(),
                    b"jpeg".as_slice(),
                ]
                .iter()
                .any(|supported| kind.eq_ignore_ascii_case(supported))
                && let Some((0x04, picture, _)) = der_field(fields)
            {
                return Some((kind, picture));
            }
            if let Some(picture) = seal_picture(value, depth + 1) {
                return Some(picture);
            }
        }
        data = rest;
    }
    None
}

/// Seal containers are ASN.1 envelopes with a complete PNG or JPEG embedded.
/// Extract the first complete image without depending on a specific vendor's
/// ASN.1 layout.
fn extract_embedded_image(data: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    if let Some(start) = find_bytes(data, b"\x89PNG\r\n\x1a\n") {
        let mut position = start + 8;
        while position + 12 <= data.len() {
            let length = u32::from_be_bytes([
                data[position],
                data[position + 1],
                data[position + 2],
                data[position + 3],
            ]) as usize;
            let kind = &data[position + 4..position + 8];
            position += 12 + length;
            if position > data.len() {
                break;
            }
            if kind == b"IEND" {
                return Some((data[start..position].to_vec(), "image/png"));
            }
        }
    }
    if let Some(start) = find_bytes(data, b"\xff\xd8\xff") {
        let mut position = start + 2;
        while position + 1 < data.len() {
            if data[position] == 0xff {
                let marker = data[position + 1];
                if marker == 0xd9 {
                    return Some((data[start..position + 2].to_vec(), "image/jpeg"));
                }
                if marker == 0xda {
                    // Skip entropy-coded data until the next marker.
                    position += 2;
                    while position + 1 < data.len() {
                        if data[position] == 0xff && data[position + 1] != 0x00 {
                            break;
                        }
                        position += 1;
                    }
                    continue;
                }
                if position + 4 <= data.len() {
                    let length =
                        u16::from_be_bytes([data[position + 2], data[position + 3]]) as usize;
                    position += 2 + length;
                    continue;
                }
            }
            position += 1;
        }
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod signature_tests {
    use super::{
        OfdDocument, decode_seal_appearance, der_field, extract_embedded_image, seal_picture,
    };
    use crate::{PageRenderer, RasterRegion};

    #[test]
    fn signed_value_ofd_seal_is_visible_and_transparent() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/pdf-match/1.ofd");
        let doc = OfdDocument::open(&path).unwrap();
        let stamps = doc.signature_seals();
        assert_eq!(stamps.len(), 1);
        assert_eq!(
            (stamps[0].x, stamps[0].y, stamps[0].width, stamps[0].height),
            (90.0, 8.0, 30.0, 20.0)
        );
        assert_eq!(stamps[0].mime, "image/svg+xml");
        let svg = std::str::from_utf8(&stamps[0].data).unwrap();
        assert!(svg.contains("viewBox=\"0 0 30 20\""));
        assert!(
            !svg.contains("fill=\"#FFFFFF\""),
            "no artificial paper background on a seal"
        );

        // Same document with signature appearances disabled: detect actual
        // added red ink, not merely the presence of an <image> in exported SVG.
        let bare = OfdDocument::from_seal_bytes(std::fs::read(path).unwrap()).unwrap();
        let renderer = PageRenderer::new(&doc);
        let baseline = PageRenderer::new(&bare);
        for dpi in [96.0, 384.0, 768.0] {
            let region = RasterRegion {
                x: (90.0 * dpi / 25.4) as u32,
                y: (8.0 * dpi / 25.4) as u32,
                width: (30.0 * dpi / 25.4_f32).ceil() as u32,
                height: (20.0 * dpi / 25.4_f32).ceil() as u32,
            };
            let actual = renderer.rasterize_region(&doc, 0, dpi, region).unwrap();
            let before = baseline.rasterize_region(&bare, 0, dpi, region).unwrap();
            let red_pixels = |data: &[u8]| {
                data.chunks_exact(4)
                    .filter(|p| p[2] > 150 && p[1] < 100 && p[0] < 100 && p[3] > 150)
                    .count()
            };
            assert!(
                red_pixels(&actual.bgra) > red_pixels(&before.bgra) + 100,
                "seal ink missing at {dpi} DPI"
            );
            // A transparent corner must leave the underlying page untouched.
            assert_eq!(&actual.bgra[..4], &before.bgra[..4]);
        }
    }

    #[test]
    fn der_picture_fields_are_bounded_and_typed() {
        let picture = b"\x30\x10\x16\x03ofd\x04\x03zip\x02\x01\x1e\x02\x01\x14";
        let (kind, data) = seal_picture(picture, 0).unwrap();
        assert_eq!(kind, b"ofd");
        assert_eq!(data, b"zip");
        assert!(decode_seal_appearance(picture).is_none());
        for invalid in [
            b"\x30\x80".as_slice(),
            b"\x04\x88\xff\xff\xff\xff\xff\xff\xff\xff",
            b"\x04\x04abc",
            b"\x30",
        ] {
            assert!(der_field(invalid).is_none());
            assert!(decode_seal_appearance(invalid).is_none());
        }
        assert!(seal_picture(picture, 17).is_none());
        assert!(decode_seal_appearance(b"not a picture PK\x03\x04").is_none());
    }

    #[test]
    fn extracts_png_from_an_asn1_envelope() {
        let mut data = vec![0x30, 0x82, 0x01, 0x00];
        data.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        // IHDR chunk with no payload, then IEND; extraction only scans chunks.
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"IEND");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&[0, 1, 2]);
        let (image, mime) = extract_embedded_image(&data).unwrap();
        assert_eq!(mime, "image/png");
        assert!(image.starts_with(b"\x89PNG"));
        assert!(image.ends_with(&[0, 0, 0, 0]));
    }

    #[test]
    fn returns_none_without_an_embedded_image() {
        assert!(extract_embedded_image(b"not an image").is_none());
    }
}
