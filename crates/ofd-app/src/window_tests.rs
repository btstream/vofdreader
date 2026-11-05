use super::{Corners, Decorations, Tiling, content_corners, px};

#[test]
fn floating_content_has_four_concentric_corners() {
    assert_eq!(
        content_corners(
            Decorations::Client {
                tiling: Tiling::default()
            },
            false
        ),
        Corners::all(px(4.0)),
    );
}

#[test]
fn tiled_edges_square_only_the_adjacent_corners() {
    let corners = content_corners(
        Decorations::Client {
            tiling: Tiling {
                left: true,
                ..Default::default()
            },
        },
        false,
    );
    assert_eq!(corners.top_left, px(0.0));
    assert_eq!(corners.bottom_left, px(0.0));
    assert_eq!(corners.top_right, px(4.0));
    assert_eq!(corners.bottom_right, px(4.0));
}

#[test]
fn maximized_fullscreen_and_server_decorations_are_square() {
    let maximized = Decorations::Client {
        tiling: Tiling {
            top: true,
            bottom: true,
            left: true,
            right: true,
        },
    };
    assert_eq!(content_corners(maximized, false), Corners::default());
    assert_eq!(
        content_corners(
            Decorations::Client {
                tiling: Tiling::default()
            },
            true
        ),
        Corners::default()
    );
    assert_eq!(
        content_corners(Decorations::Server, false),
        Corners::default()
    );
}
