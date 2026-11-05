use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ofd_core::OfdDocument;

#[derive(Parser)]
#[command(
    name = "ofd-cli",
    version,
    about = "检查与调试 OFD 文件（GB/T 33190-2016）"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 显示文档元数据、页面尺寸、资源与签名概况
    Info { file: PathBuf },
    /// 列出页面及每页对象统计
    Pages { file: PathBuf },
    /// 提取页面文字（默认全部页面）
    Text {
        file: PathBuf,
        /// 从 0 开始的页码
        #[arg(short, long)]
        page: Option<usize>,
    },
    /// 执行结构检查并列出非致命问题
    Check { file: PathBuf },
    /// 核验实际内嵌字形使用及回退数量（默认全部页面）
    Fonts {
        file: PathBuf,
        #[arg(short, long)]
        page: Option<usize>,
        /// 存在内嵌字体回退时返回失败，供回归检查使用
        #[arg(long)]
        strict: bool,
    },
    /// 将页面导出为 SVG 或 PNG
    Render {
        file: PathBuf,
        /// 从 0 开始的页码
        #[arg(short, long, default_value_t = 0)]
        page: usize,
        /// SVG 输出路径；不指定 --png 时默认输出到 stdout
        #[arg(long)]
        svg: Option<PathBuf>,
        /// PNG 输出路径
        #[arg(long)]
        png: Option<PathBuf>,
        /// PNG 分辨率
        #[arg(long, default_value_t = 144.0)]
        dpi: f32,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> ofd_core::Result<()> {
    match cli.command {
        Command::Info { file } => info(&file),
        Command::Pages { file } => pages(&file),
        Command::Text { file, page } => text(&file, page),
        Command::Check { file } => check(&file),
        Command::Fonts { file, page, strict } => {
            let doc = OfdDocument::open(&file)?;
            let renderer = ofd_core::PageRenderer::new(&doc);
            let pages: Vec<_> =
                page.map_or_else(|| (0..doc.page_count()).collect(), |page| vec![page]);
            let mut replacements = 0;
            for page in pages {
                let report = renderer.font_report(&doc, page)?;
                replacements += report.fallback_chars;
                println!(
                    "#{} 内嵌字形={} 回退字符={} 回退字体={}",
                    page + 1,
                    report.embedded_glyphs,
                    report.fallback_chars,
                    report.replaced_fonts.join(",")
                );
                for (font, codepoints) in &report.fallback_codepoints {
                    println!(
                        "  {font}: {}",
                        codepoints
                            .iter()
                            .map(|code| format!("U+{code:04X}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
            }
            if strict && replacements > 0 {
                return Err(ofd_core::OfdError::Render(format!(
                    "{replacements} 个字符未使用文档内嵌字形"
                )));
            }
            Ok(())
        }
        Command::Render {
            file,
            page,
            svg,
            png,
            dpi,
        } => render(&file, page, svg.as_deref(), png.as_deref(), dpi),
    }
}

fn info(file: &PathBuf) -> ofd_core::Result<()> {
    let doc = OfdDocument::open(file)?;
    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);

    println!("VofdReader · ofd-cli {}", env!("CARGO_PKG_VERSION"));
    println!(
        "包：{}（{}）",
        file.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.display().to_string()),
        human_size(size)
    );
    println!("格式：OFD {} / {}", doc.ofd.version, doc.ofd.doc_type);
    if let Some(title) = &doc.ofd.info.title {
        println!("标题：{title}");
    }
    if let Some(creator) = &doc.ofd.info.creator {
        println!("创建者：{creator}");
    }
    if let Some(date) = &doc.ofd.info.creation_date {
        println!("创建日期：{date}");
    }
    println!("入口：{}", doc.entry);
    println!("文档根：{}", doc.document_file);
    println!("页面：{} 页", doc.page_count());

    for index in 0..doc.page_count() {
        match doc.page_summary(index) {
            Ok(summary) => {
                let size = summary
                    .size
                    .map(|size| size.to_string())
                    .unwrap_or_else(|| "尺寸未知".to_string());
                let templates = if summary.templates.is_empty() {
                    String::new()
                } else {
                    format!("，模板 {}", summary.templates.join(", "))
                };
                println!(
                    "  #{:<2} id={:<6} {:<22} 对象 {}（文字 {} / 图形 {} / 图像 {}{}）{}",
                    index + 1,
                    summary.id,
                    size,
                    summary.object_count(),
                    summary.text_objects,
                    summary.path_objects,
                    summary.image_objects,
                    if summary.page_blocks > 0 {
                        format!(" / 页块 {}", summary.page_blocks)
                    } else {
                        String::new()
                    },
                    templates
                );
            }
            Err(error) => println!("  #{:<2} 读取失败：{error}", index + 1),
        }
    }

    println!(
        "资源：字体 {} 个（内嵌 {}），图像 {} 个，绘制参数 {}，色彩空间 {}",
        doc.resources.fonts.len(),
        doc.resources.embedded_font_count(),
        doc.resources.images.len(),
        doc.resources.draw_params.len(),
        doc.resources.color_spaces
    );

    for font in &doc.resources.fonts {
        println!(
            "  字体 {:<5} {:<16} {:<10} {}",
            font.id,
            if font.family.is_empty() {
                &font.name
            } else {
                &font.family
            },
            font.scope,
            if let Some(file) = &font.file {
                format!("内嵌 {file}")
            } else {
                "系统字体 / 回退".to_string()
            }
        );
    }

    let annot_count: usize = doc.annotations.iter().map(|page| page.annots.len()).sum();
    let mut kinds = std::collections::BTreeMap::new();
    let mut appearance_images = 0;
    let mut appearance_texts = 0;
    for page in &doc.annotations {
        for annot in &page.annots {
            *kinds.entry(annot.kind.clone()).or_insert(0) += 1;
            appearance_images += annot.image_resources.len();
            appearance_texts += annot.text.len();
        }
    }
    println!(
        "注释：{} 个{}",
        annot_count,
        if kinds.is_empty() {
            String::new()
        } else {
            format!(
                "（{}；外观图像 {}，外观文字 {}）",
                kinds
                    .iter()
                    .map(|(kind, count)| format!("{kind} × {count}"))
                    .collect::<Vec<_>>()
                    .join("，"),
                appearance_images,
                appearance_texts
            )
        }
    );

    println!(
        "签名：{}",
        if doc.has_signature_material() {
            "检测到签名材料（尚未验证）"
        } else {
            "无"
        }
    );

    if !doc.warnings.is_empty() {
        println!("警告：{} 条", doc.warnings.len());
        for warning in &doc.warnings {
            println!("  - {warning}");
        }
    }

    Ok(())
}

fn pages(file: &PathBuf) -> ofd_core::Result<()> {
    let doc = OfdDocument::open(file)?;
    println!("共 {} 页", doc.page_count());
    for index in 0..doc.page_count() {
        let page = doc.page(index)?;
        let summary = doc.page_summary(index)?;
        println!(
            "#{:<3} id={:<8} {:<22} 文字 {:<3} 图形 {:<3} 图像 {:<3} CGTransform {:<3} 模板 {}",
            index + 1,
            page.id,
            summary
                .size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "?".to_string()),
            summary.text_objects,
            summary.path_objects,
            summary.image_objects,
            summary.cg_transforms,
            if summary.templates.is_empty() {
                "-".to_string()
            } else {
                summary.templates.join(",")
            }
        );
        for warning in &summary.warnings {
            println!("      警告：{warning}");
        }
    }
    Ok(())
}

fn text(file: &PathBuf, page: Option<usize>) -> ofd_core::Result<()> {
    let doc = OfdDocument::open(file)?;
    let indices: Vec<usize> = match page {
        Some(index) => {
            doc.page(index)?;
            vec![index]
        }
        None => (0..doc.page_count()).collect(),
    };

    for index in indices {
        let lines = doc.page_text(index)?;
        println!("=== 第 {} 页（{} 行）===", index + 1, lines.len());
        for (line_index, line) in lines.iter().enumerate() {
            println!("{:>3}: {}", line_index + 1, line);
        }
    }
    Ok(())
}

fn check(file: &PathBuf) -> ofd_core::Result<()> {
    let doc = OfdDocument::open(file)?;
    let issues = doc.check();
    if issues.is_empty() {
        println!("结构检查通过，未发现问题。");
    } else {
        println!("结构检查完成，共 {} 条提示：", issues.len());
        for issue in issues {
            println!("  - {issue}");
        }
    }
    Ok(())
}

fn render(
    file: &PathBuf,
    page: usize,
    svg_path: Option<&std::path::Path>,
    png_path: Option<&std::path::Path>,
    dpi: f32,
) -> ofd_core::Result<()> {
    let doc = OfdDocument::open(file)?;
    doc.page(page)?;

    if let Some(path) = svg_path {
        let svg = doc.page_svg(page)?;
        std::fs::write(path, svg)?;
        println!("SVG → {}", path.display());
    } else if png_path.is_none() {
        print!("{}", doc.page_svg(page)?);
    }

    if let Some(path) = png_path {
        let rendered = doc.render_page_png(page, dpi)?;
        std::fs::write(path, rendered.png)?;
        println!(
            "PNG → {}（{} × {}，{dpi} DPI）",
            path.display(),
            rendered.width,
            rendered.height
        );
    }

    Ok(())
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
