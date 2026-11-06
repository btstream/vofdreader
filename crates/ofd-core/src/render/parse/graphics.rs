use crate::document::OfdDocument;
use crate::model::Color;
use crate::xml::{child, child_text, children, parse_xml};

use super::super::types::*;
use super::text::*;

pub(crate) fn walk_block(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    items: &mut Vec<Item>,
    inherited: &PathStyle,
    warnings: &mut Vec<String>,
) {
    for child_node in node.children().filter(|node| node.is_element()) {
        let name = child_node.tag_name().name();
        let mut style = inherited.clone();
        if let Some(id) = child_node.attribute("DrawParam")
            && let Some(param) = doc.resources.draw_param(id)
        {
            style.apply(&param);
        }

        let start = items.len();
        match name {
            "Layer" | "PageBlock" => {
                walk_block(doc, child_node, items, &style, warnings);
            }
            "TextObject" => {
                if let Some(item) = parse_text_object(doc, child_node, &style) {
                    items.push(item);
                }
            }
            "PathObject" => {
                if let Some(expanded) = expand_pattern_fill(doc, child_node, &style, warnings) {
                    items.extend(expanded);
                } else if let Some(item) = parse_path_object(doc, child_node, &style, warnings) {
                    items.push(item);
                }
            }
            "ImageObject" => {
                if let Some(item) = parse_image_object(doc, child_node, warnings) {
                    items.push(item);
                }
            }
            "CompositeObject" => {
                append_composite_unit(doc, child_node, &style, items, warnings);
            }
            _ => {}
        }

        if name != "Layer"
            && name != "PageBlock"
            && items.len() > start
            && let Some(regions) = parse_clips(child_node, warnings)
        {
            let clipped = items.split_off(start);
            items.push(Item::Clip {
                regions,
                items: clipped,
            });
        }
    }
}

/// Object `Clips` as parent-space regions.
///
/// Clip geometry lives in the object's Boundary space:
/// `T(Boundary) · Area.CTM · T(Path.Boundary) · Path.CTM`. With
/// `TransFlag="true"` it is also transformed by the object CTM (producers
/// then write e.g. a unit square for an image). Each `Clip` is one region
/// (its `Area`s unite); regions intersect. Text areas are not supported and
/// leave their region out, which never hides content.
pub(crate) fn parse_clips(
    node: roxmltree::Node<'_, '_>,
    warnings: &mut Vec<String>,
) -> Option<Vec<Vec<ClipShape>>> {
    let clips = child(node, "Clips")?;
    let (x, y) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .map(|(x, y, _, _)| (x, y))
        .unwrap_or((0.0, 0.0));
    let mut origin = Matrix::translate(x, y);
    if clips
        .attribute("TransFlag")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        && let Some(ctm) = node.attribute("CTM").and_then(Matrix::parse)
    {
        origin = origin.compose(ctm);
    }
    let mut regions = Vec::new();
    for clip in children(clips, "Clip") {
        let mut region = Vec::new();
        let mut complete = true;
        for area in children(clip, "Area") {
            let area_ctm = area
                .attribute("CTM")
                .and_then(Matrix::parse)
                .unwrap_or(Matrix::IDENTITY);
            let Some(path) = child(area, "Path") else {
                complete = false;
                continue;
            };
            let Some(cmds) = child_text(path, "AbbreviatedData")
                .and_then(|data| parse_path_data(data, warnings))
                .filter(|cmds| !cmds.is_empty())
            else {
                complete = false;
                continue;
            };
            let (path_x, path_y) = path
                .attribute("Boundary")
                .and_then(crate::model::parse_box)
                .map(|(x, y, _, _)| (x, y))
                .unwrap_or((0.0, 0.0));
            let path_ctm = path
                .attribute("CTM")
                .and_then(Matrix::parse)
                .unwrap_or(Matrix::IDENTITY);
            region.push(ClipShape {
                transform: origin
                    .compose(area_ctm)
                    .compose(Matrix::translate(path_x, path_y))
                    .compose(path_ctm),
                cmds,
                even_odd: path
                    .attribute("Rule")
                    .is_some_and(|value| value.eq_ignore_ascii_case("Even-Odd")),
            });
        }
        if !complete {
            warnings.push("裁剪区含不支持的文字或无效路径，已忽略该裁剪区".to_string());
        } else if !region.is_empty() {
            regions.push(region);
        }
    }
    (!regions.is_empty()).then_some(regions)
}

/// `AxialShd` / `RadialShd` inside a `FillColor` or `StrokeColor`.
pub(crate) fn parse_shading(color: roxmltree::Node<'_, '_>) -> Option<Shading> {
    let node = child(color, "AxialShd").or_else(|| child(color, "RadialShd"))?;
    let point = |name: &str| -> Option<(f64, f64)> {
        let mut values = node.attribute(name)?.split_whitespace();
        Some((values.next()?.parse().ok()?, values.next()?.parse().ok()?))
    };
    let kind = if node.tag_name().name() == "AxialShd" {
        ShadingKind::Axial {
            start: point("StartPoint")?,
            end: point("EndPoint")?,
        }
    } else {
        ShadingKind::Radial {
            start: point("StartPoint")?,
            start_radius: attribute_f64(node, "StartRadius").unwrap_or(0.0).max(0.0),
            end: point("EndPoint")?,
            radius: attribute_f64(node, "EndRadius")?,
        }
    };
    let segments: Vec<_> = children(node, "Segment").collect();
    let count = segments.len();
    let mut stops = Vec::with_capacity(count);
    for (index, segment) in segments.into_iter().enumerate() {
        let Some(color) = child(segment, "Color") else {
            continue;
        };
        let Some(value) = color.attribute("Value").and_then(|value| {
            Color::parse(
                value,
                color
                    .attribute("Alpha")
                    .and_then(|alpha| alpha.parse::<u8>().ok())
                    .unwrap_or(255),
            )
        }) else {
            continue;
        };
        let default = if count > 1 {
            index as f64 / (count - 1) as f64
        } else {
            0.0
        };
        let position = attribute_f64(segment, "Position")
            .filter(|position| position.is_finite())
            .unwrap_or(default)
            .clamp(0.0, 1.0);
        stops.push((position, value));
    }
    if stops.is_empty() {
        return None;
    }
    // Keep document order for equal positions (hard colour steps).
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let extend = node
        .attribute("Extend")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    let spread = match node.attribute("MapType") {
        Some(value) if value.eq_ignore_ascii_case("Repeat") => ShadingSpread::Repeat,
        Some(value) if value.eq_ignore_ascii_case("Reflect") => ShadingSpread::Reflect,
        _ => ShadingSpread::Pad,
    };
    Some(Shading {
        kind,
        stops,
        extend_start: extend & 1 != 0,
        extend_end: extend & 2 != 0,
        spread,
    })
}

/// `CompositeObject` references a reusable `CompositeGraphicUnit` resource.
/// The unit content is stored as a standalone XML fragment; parse it here and
/// wrap the resulting items in the object's Boundary + CTM.
pub(crate) fn append_composite_unit(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
    items: &mut Vec<Item>,
    warnings: &mut Vec<String>,
) {
    let Some(resource_id) = node.attribute("ResourceID") else {
        return;
    };
    let Some(unit) = doc.resources.composite_unit(resource_id) else {
        warnings.push(format!("复合图形资源 {resource_id} 不存在"));
        return;
    };
    let document = match parse_xml(unit.content.as_bytes(), "CompositeGraphicUnit") {
        Ok(document) => document,
        Err(error) => {
            warnings.push(format!("复合图形资源 {} 解析失败：{error}", unit.id));
            return;
        }
    };
    let root = document.root_element();
    let unit_root = if root.tag_name().name() == "CompositeGraphicUnit" {
        root
    } else {
        root.descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "CompositeGraphicUnit")
            .unwrap_or(root)
    };

    let mut unit_items = Vec::new();
    let mut found_content = false;
    for content in unit_root
        .children()
        .filter(|child| child.is_element() && child.tag_name().name() == "Content")
    {
        found_content = true;
        walk_block(doc, content, &mut unit_items, inherited, warnings);
    }
    if !found_content {
        walk_block(doc, unit_root, &mut unit_items, inherited, warnings);
    }
    if !unit_items.is_empty() {
        let opacity = node
            .attribute("Alpha")
            .and_then(|value| value.parse::<u16>().ok())
            .map(|alpha| alpha.min(255) as f64 / 255.0)
            .unwrap_or(1.0);
        items.push(Item::Group {
            transform: object_transform(node),
            items: unit_items,
            opacity,
        });
    }
}

pub(crate) fn object_transform(node: roxmltree::Node<'_, '_>) -> Matrix {
    let (x, y) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .map(|(x, y, _, _)| (x, y))
        .unwrap_or((0.0, 0.0));
    let ctm = node
        .attribute("CTM")
        .and_then(Matrix::parse)
        .unwrap_or(Matrix::IDENTITY);
    Matrix::translate(x, y).compose(ctm)
}

/// OFD paints a pattern-filled path by tiling `CellContent`. A solid fill
/// fallback would paint the whole page black (the watermark test does exactly
/// that), so expand the tiles into groups instead.
pub(crate) fn expand_pattern_fill(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
    warnings: &mut Vec<String>,
) -> Option<Vec<Item>> {
    let fill = child(node, "FillColor")?;
    let pattern = child(fill, "Pattern")?;
    let cell = child(pattern, "CellContent")?;
    let width = attribute_f64(pattern, "Width")?;
    let height = attribute_f64(pattern, "Height")?;
    let x_step = attribute_f64(pattern, "XStep").unwrap_or(width);
    let y_step = attribute_f64(pattern, "YStep").unwrap_or(height);
    if width <= 0.0 || height <= 0.0 || x_step <= 1e-6 || y_step <= 1e-6 {
        return None;
    }
    let ctm = pattern
        .attribute("CTM")
        .and_then(Matrix::parse)
        .unwrap_or(Matrix::IDENTITY);
    let inverse = ctm.invert()?;

    let (x, y, box_width, box_height) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .unwrap_or((0.0, 0.0, 210.0, 297.0));

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (corner_x, corner_y) in [
        (x, y),
        (x + box_width, y),
        (x + box_width, y + box_height),
        (x, y + box_height),
    ] {
        let (pattern_x, pattern_y) = inverse.transform_point(corner_x, corner_y);
        min_x = min_x.min(pattern_x);
        min_y = min_y.min(pattern_y);
        max_x = max_x.max(pattern_x);
        max_y = max_y.max(pattern_y);
    }

    let mut cell_items = Vec::new();
    walk_block(doc, cell, &mut cell_items, inherited, warnings);
    if cell_items.is_empty() {
        return None;
    }

    let start_column = (min_x / x_step).floor() as i64 - 1;
    let end_column = (max_x / x_step).ceil() as i64 + 1;
    let start_row = (min_y / y_step).floor() as i64 - 1;
    let end_row = (max_y / y_step).ceil() as i64 + 1;
    let columns = (end_column - start_column + 1).max(0) as usize;
    let rows = (end_row - start_row + 1).max(0) as usize;
    if columns.saturating_mul(rows) > 4096 {
        warnings.push("图案平铺数量过多，已跳过".to_string());
        return None;
    }

    let mut items = Vec::with_capacity(columns * rows);
    for row in start_row..=end_row {
        for column in start_column..=end_column {
            let tile = ctm.compose(Matrix::translate(
                column as f64 * x_step,
                row as f64 * y_step,
            ));
            items.push(Item::Group {
                transform: tile,
                items: cell_items.clone(),
                opacity: 1.0,
            });
        }
    }
    Some(items)
}

pub(crate) fn attribute_f64(node: roxmltree::Node<'_, '_>, name: &str) -> Option<f64> {
    node.attribute(name).and_then(|value| value.parse().ok())
}

pub(crate) fn parse_path_object(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
    warnings: &mut Vec<String>,
) -> Option<Item> {
    let mut style = inherited.clone();
    if let Some(id) = node.attribute("DrawParam")
        && let Some(param) = doc.resources.draw_param(id)
    {
        style.apply(&param);
    }

    // OFD 8.2 表 41: a path is stroked by default and is **not** filled.
    // FillColor alone (including a colour inherited from a layer DrawParam)
    // must not turn on filling; text objects have the opposite defaults.
    style.fill = node
        .attribute("Fill")
        .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
    style.stroke = node
        .attribute("Stroke")
        .is_none_or(|value| !(value.eq_ignore_ascii_case("false") || value == "0"));
    if let Some(width) = node
        .attribute("LineWidth")
        .and_then(|value| value.parse::<f64>().ok())
    {
        style.line_width = width;
    }
    if let Some(cap) = node.attribute("Cap") {
        style.cap = cap.to_string();
    }
    if let Some(join) = node.attribute("Join") {
        style.join = join.to_string();
    }
    if let Some(dash) = node.attribute("DashPattern") {
        style.dash = dash
            .split_whitespace()
            .filter_map(|part| part.parse().ok())
            .collect();
    }
    if node
        .attribute("Rule")
        .is_some_and(|value| value.eq_ignore_ascii_case("Even-Odd"))
    {
        style.even_odd = true;
    }
    if let Some(color) = child(node, "FillColor")
        && let Some(value) = color.attribute("Value")
    {
        style.fill_color = Color::parse(
            value,
            color
                .attribute("Alpha")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(255),
        );
    }
    style.fill_shading = child(node, "FillColor").and_then(parse_shading);
    style.stroke_shading = child(node, "StrokeColor").and_then(parse_shading);
    if let Some(color) = child(node, "StrokeColor")
        && let Some(value) = color.attribute("Value")
    {
        style.stroke_color = Color::parse(
            value,
            color
                .attribute("Alpha")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(255),
        );
    }
    if let Some(alpha) = node
        .attribute("Alpha")
        .and_then(|value| value.parse::<u16>().ok())
    {
        style.opacity = alpha.min(255) as f64 / 255.0;
    }

    let data = child_text(node, "AbbreviatedData")?;
    let cmds = parse_path_data(data, warnings)?;
    if cmds.is_empty() {
        return None;
    }

    Some(Item::Path {
        transform: object_transform(node),
        cmds,
        style,
    })
}

pub(crate) fn parse_path_data(data: &str, warnings: &mut Vec<String>) -> Option<Vec<PathCmd>> {
    let mut tokens = data.split_whitespace();
    let mut cmds = Vec::new();

    while let Some(token) = tokens.next() {
        let mut number = |what: &str| -> Option<f64> {
            let value = tokens.next()?;
            match value.parse::<f64>() {
                Ok(value) => Some(value),
                Err(_) => {
                    warnings.push(format!("路径 {what} 参数非法：{value}"));
                    None
                }
            }
        };

        match token {
            "S" | "M" => {
                let x = number("M")?;
                let y = number("M")?;
                cmds.push(PathCmd::Move(x, y));
            }
            "L" => {
                let x = number("L")?;
                let y = number("L")?;
                cmds.push(PathCmd::Line(x, y));
            }
            "Q" => {
                let x1 = number("Q")?;
                let y1 = number("Q")?;
                let x2 = number("Q")?;
                let y2 = number("Q")?;
                cmds.push(PathCmd::Quad(x1, y1, x2, y2));
            }
            "B" => {
                let x1 = number("B")?;
                let y1 = number("B")?;
                let x2 = number("B")?;
                let y2 = number("B")?;
                let x3 = number("B")?;
                let y3 = number("B")?;
                cmds.push(PathCmd::Cubic(x1, y1, x2, y2, x3, y3));
            }
            "A" => {
                let rx = number("A")?;
                let ry = number("A")?;
                let rotation = number("A")?;
                let large = number("A")? != 0.0;
                let sweep = number("A")? != 0.0;
                let x = number("A")?;
                let y = number("A")?;
                cmds.push(PathCmd::Arc {
                    rx,
                    ry,
                    rotation,
                    large,
                    sweep,
                    x,
                    y,
                });
            }
            "C" => cmds.push(PathCmd::Close),
            other => {
                warnings.push(format!("不支持的路径操作符：{other}"));
                return None;
            }
        }
    }

    Some(cmds)
}

pub(crate) fn parse_image_object(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    warnings: &mut Vec<String>,
) -> Option<Item> {
    let resource_id = node.attribute("ResourceID")?;
    let Some(image) = doc.resources.image(resource_id) else {
        warnings.push(format!("图像资源未声明：{resource_id}"));
        return None;
    };
    let Some(file) = &image.file else {
        return None;
    };
    let data = match doc.package.read(file) {
        Ok(data) => data,
        Err(error) => {
            warnings.push(format!("图像读取失败 {file}：{error}"));
            return None;
        }
    };

    let (_, _, width, height) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .unwrap_or((0.0, 0.0, 1.0, 1.0));
    let ctm = node.attribute("CTM").and_then(Matrix::parse);
    let has_ctm = ctm.is_some_and(|matrix| !matrix.is_identity());
    let (local_width, local_height) = if has_ctm { (1.0, 1.0) } else { (width, height) };

    let mime = match image.format.as_deref() {
        Some(format) if format.eq_ignore_ascii_case("JPEG") => "image/jpeg",
        Some(format) if format.eq_ignore_ascii_case("PNG") => "image/png",
        _ => detect_image_mime(&data),
    };

    Some(Item::Image {
        transform: object_transform(node),
        width: local_width,
        height: local_height,
        mime,
        data,
        blend: node.attribute("BlendMode").map(str::to_string),
        clip: None,
    })
}

pub(crate) fn blend_mode(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "darken" => Some("darken"),
        "multiply" => Some("multiply"),
        "screen" => Some("screen"),
        "overlay" => Some("overlay"),
        "lighten" => Some("lighten"),
        _ => None,
    }
}

pub(crate) fn detect_image_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"BM") {
        "image/bmp"
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        "image/tiff"
    } else {
        "image/png"
    }
}
