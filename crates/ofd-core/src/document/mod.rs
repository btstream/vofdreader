use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use crate::content::parse_page_content;
use crate::error::{OfdError, Result};
use crate::loc::{join_dir, resolve_loc};
use crate::model::{
    AnnotInfo, AnnotationPage, Color, CompositeUnit, DocInfo, DocumentMeta, DrawParam,
    FontResource, ImageResource, OfdInfo, PageRef, PageSummary, ResScope, ResourceSet, SizeMm,
    TemplateRef, parse_physical_box,
};
use crate::package::Package;
use crate::xml::{child, child_text, children, parse_xml, required_attr};

mod signature;

use signature::load_signature_seals;
pub(crate) use signature::{RenderedSeal, SealAppearance, SealStamp};
#[cfg(test)]
pub(crate) use signature::{
    decode_seal_appearance, der_field, extract_embedded_image, seal_picture,
};

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

impl OfdDocument {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let package = Package::open(path)?;
        Self::from_package(package, true)
    }

    // Nested seal appearances must not recursively load signature appearances.
    pub(crate) fn from_seal_bytes(data: Vec<u8>) -> Result<Self> {
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

#[cfg(test)]
mod tests;
