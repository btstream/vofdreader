//! Retention estimates for the main contents of prepared pages. Shared paths/images are
//! charged once within a page, and again across pages deliberately: this bounds
//! cache admission without pretending to measure the entire renderer heap or
//! every container's spare capacity and shader/font allocation.
use super::{
    raster::{Op, Prepared},
    renderer::CachedPage,
};
use resvg::{tiny_skia as sk, usvg};
use std::collections::HashSet;
use std::mem::{size_of, size_of_val};

#[derive(Default)]
struct Estimate {
    bytes: usize,
    seen: HashSet<(u8, usize)>,
}

impl Estimate {
    fn add(&mut self, bytes: usize) {
        self.bytes = self.bytes.saturating_add(bytes);
    }
    fn path(&mut self, path: &sk::Path) {
        if self.seen.insert((0, path as *const _ as usize)) {
            self.add(size_of_val(path));
            self.add(size_of_val(path.points()));
            self.add(size_of_val(path.verbs()));
        }
    }
    fn ops(&mut self, ops: &[Op]) {
        self.add(size_of_val(ops));
        for op in ops {
            match op {
                Op::Fill { path, .. } | Op::Stroke { path, .. } => self.path(path),
                Op::Image { image, .. } => {
                    if self.seen.insert((1, image.data().as_ptr() as usize)) {
                        self.add(image.data().len());
                    }
                }
                Op::Vector { tree, .. } => self.tree(tree),
                Op::Layer { ops, .. } => self.ops(ops),
                Op::Clip { ops, regions, .. } => {
                    self.ops(ops);
                    self.add(size_of_val(regions.as_slice()));
                    for region in regions {
                        self.add(size_of_val(region.as_slice()));
                        for outline in region {
                            self.path(&outline.path);
                        }
                    }
                }
            }
        }
    }
    fn tree(&mut self, tree: &usvg::Tree) {
        self.add(size_of::<usvg::Tree>());
        self.group(tree.root());
    }
    fn group(&mut self, group: &usvg::Group) {
        if !self.seen.insert((2, group as *const _ as usize)) {
            return;
        }
        self.add(size_of_val(group));
        self.add(size_of_val(group.children()));
        for node in group.children() {
            match node {
                usvg::Node::Group(group) => self.group(group),
                usvg::Node::Path(path) => {
                    self.add(size_of_val(path.as_ref()));
                    self.path(path.data());
                }
                usvg::Node::Image(image) => {
                    self.add(size_of_val(image.as_ref()));
                    match image.kind() {
                        usvg::ImageKind::JPEG(data)
                        | usvg::ImageKind::PNG(data)
                        | usvg::ImageKind::GIF(data)
                        | usvg::ImageKind::WEBP(data) => {
                            if self.seen.insert((3, data.as_ptr() as usize)) {
                                self.add(data.len());
                            }
                        }
                        usvg::ImageKind::SVG(tree) => self.tree(tree),
                    }
                }
                usvg::Node::Text(text) => {
                    self.add(size_of_val(text.as_ref()));
                    self.group(text.flattened());
                }
            }
            node.subroots(|root| self.group(root));
        }
    }
}

impl CachedPage {
    pub(super) fn estimated_bytes(&self) -> usize {
        let mut estimate = Estimate::default();
        match self {
            Self::Prepared(page) => {
                estimate.add(size_of::<Prepared>());
                estimate.ops(&page.ops);
            }
            Self::Tree(tree) => estimate.tree(tree),
        }
        estimate.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::super::raster::ops::Bounds;
    use super::*;
    use crate::model::SizeMm;
    use std::sync::Arc;

    fn image_op(image: Arc<sk::Pixmap>) -> Op {
        Op::Image {
            image,
            ts: sk::Transform::identity(),
            rect: sk::Rect::from_xywh(0.0, 0.0, 4.0, 4.0).unwrap(),
            blend: sk::BlendMode::SourceOver,
            bounds: Bounds {
                x0: 0.0,
                y0: 0.0,
                x1: 4.0,
                y1: 4.0,
            },
        }
    }

    #[test]
    fn parsed_page_weights_include_decoded_pixels_and_deduplicate_shared_sources() {
        let image = Arc::new(sk::Pixmap::new(4, 4).unwrap());
        let page = |ops| {
            CachedPage::Prepared(Arc::new(Prepared {
                size: SizeMm::new(10.0, 10.0),
                ops,
            }))
        };
        let shared = page(vec![image_op(image.clone()), image_op(image.clone())]);
        let separate = page(vec![
            image_op(image),
            image_op(Arc::new(sk::Pixmap::new(4, 4).unwrap())),
        ]);
        assert_eq!(separate.estimated_bytes() - shared.estimated_bytes(), 64);
        assert!(shared.estimated_bytes() >= 64 + size_of::<Prepared>() + 2 * size_of::<Op>());
    }

    #[test]
    fn svg_weights_walk_paths_and_clipped_subroots_without_serializing() {
        let tree = usvg::Tree::from_str(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><defs><clipPath id="c"><rect width="10" height="10"/></clipPath></defs><g clip-path="url(#c)"><path d="M0 0L20 0L20 20Z"/></g></svg>"#, &usvg::Options::default()).unwrap();
        let page = CachedPage::Tree(Arc::new(tree));
        assert!(page.estimated_bytes() > size_of::<usvg::Tree>() + size_of::<usvg::Group>());
        assert_eq!(page.estimated_bytes(), page.estimated_bytes());
    }
}
