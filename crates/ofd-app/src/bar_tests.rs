use super::pointer_moved;
use super::{Bounds, near_floating_bar, size};
use super::{point, px};

#[test]
fn only_bottom_center_of_reading_area_reveals_toolbar() {
    let bounds = Bounds {
        origin: point(px(250.0), px(100.0)),
        size: size(px(1000.0), px(700.0)),
    };
    assert!(near_floating_bar(bounds, point(px(750.0), px(760.0))));
    assert!(near_floating_bar(bounds, point(px(560.0), px(720.0))));
    assert!(
        !near_floating_bar(bounds, point(px(750.0), px(650.0))),
        "document center"
    );
    assert!(
        !near_floating_bar(bounds, point(px(300.0), px(760.0))),
        "bottom left"
    );
    assert!(
        !near_floating_bar(bounds, point(px(150.0), px(760.0))),
        "sidebar"
    );
    assert!(
        !near_floating_bar(bounds, point(px(750.0), px(810.0))),
        "status bar"
    );
}

#[test]
fn reveal_zone_stays_inside_a_narrow_reading_area() {
    let bounds = Bounds {
        origin: point(px(250.0), px(100.0)),
        size: size(px(200.0), px(300.0)),
    };
    assert!(near_floating_bar(bounds, point(px(350.0), px(360.0))));
    assert!(!near_floating_bar(bounds, point(px(240.0), px(360.0))));
    assert!(!near_floating_bar(bounds, point(px(460.0), px(360.0))));
}

#[test]
fn sub_pixel_jitter_does_not_count_as_activity() {
    assert!(!pointer_moved(
        point(px(10.0), px(10.0)),
        point(px(10.0), px(10.0))
    ));
    assert!(!pointer_moved(
        point(px(10.0), px(10.0)),
        point(px(12.0), px(12.0))
    ));
    assert!(!pointer_moved(
        point(px(10.0), px(10.0)),
        point(px(10.5), px(11.5))
    ));
}

#[test]
fn three_pixel_movement_counts_as_activity() {
    assert!(pointer_moved(
        point(px(10.0), px(10.0)),
        point(px(13.0), px(10.0))
    ));
    assert!(pointer_moved(
        point(px(10.0), px(10.0)),
        point(px(10.0), px(13.5))
    ));
    assert!(pointer_moved(
        point(px(10.0), px(10.0)),
        point(px(12.5), px(12.5))
    ));
}
