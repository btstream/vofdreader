use super::*;

#[test]
fn svg_extension_selects_svg_case_insensitively() {
    assert_eq!(
        ExportFormat::for_path(Path::new("a.SVG")),
        ExportFormat::Svg
    );
    assert_eq!(
        ExportFormat::for_path(Path::new("a.svg")),
        ExportFormat::Svg
    );
}

#[test]
fn other_or_missing_extensions_fall_back_to_png() {
    assert_eq!(
        ExportFormat::for_path(Path::new("a.png")),
        ExportFormat::Png
    );
    assert_eq!(ExportFormat::for_path(Path::new("a")), ExportFormat::Png);
    assert_eq!(
        ExportFormat::for_path(Path::new("a.pdf")),
        ExportFormat::Png
    );
}

#[test]
fn suggested_name_includes_one_based_page_and_keeps_stem() {
    assert_eq!(
        suggested_name_for("发票", 0, ExportFormat::Png),
        "发票-第1页.png"
    );
    assert_eq!(
        suggested_name_for(" invoice ", 4, ExportFormat::Svg),
        "invoice-第5页.svg"
    );
    assert_eq!(
        suggested_name_for("", 2, ExportFormat::Png),
        "ofd-第3页.png"
    );
}
