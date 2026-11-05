use super::pointer_moved;
use super::{Bounds, near_floating_bar, size};
use super::{ScrollDelta, point, px, wheel_zoom_factor};

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

#[test]
fn wheel_zoom_follows_the_wheel_direction_and_is_capped() {
    let notch = wheel_zoom_factor(ScrollDelta::Lines(point(0.0, 1.0)));
    assert!((notch - 1.1).abs() < 0.001, "wheel up zooms in");
    assert!(wheel_zoom_factor(ScrollDelta::Lines(point(0.0, -1.0))) < 1.0);

    let flick = wheel_zoom_factor(ScrollDelta::Lines(point(0.0, 100.0)));
    assert!((flick - 1.25).abs() < 0.001, "fast flicks are capped");

    let pixel_notch = wheel_zoom_factor(ScrollDelta::Pixels(point(px(0.0), px(40.0))));
    assert!(
        (pixel_notch - 1.1).abs() < 0.001,
        "precise deltas use pixels"
    );
}
