use super::*;

#[test]
fn converts_millimetres_to_page_fractions() {
    let highlights = normalize(
        SizeMm::new(200.0, 100.0),
        &[TextMatchRect {
            x: 20.0,
            y: 10.0,
            width: 40.0,
            height: 5.0,
        }],
    );
    assert_eq!(highlights.len(), 1);
    assert!((highlights[0].x - 0.1).abs() < 1e-6);
    assert!((highlights[0].width - 0.2).abs() < 1e-6);
    assert!((highlights[0].height - 0.05).abs() < 1e-6);
}

#[test]
fn clamps_rects_to_the_page() {
    let highlights = normalize(
        SizeMm::new(100.0, 100.0),
        &[
            TextMatchRect {
                x: -10.0,
                y: 90.0,
                width: 30.0,
                height: 30.0,
            },
            TextMatchRect {
                x: 120.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        ],
    );
    assert_eq!(highlights.len(), 1);
    assert_eq!(highlights[0].x, 0.0);
    assert!((highlights[0].width - 0.2).abs() < 1e-6);
    assert!((highlights[0].height - 0.1).abs() < 1e-6);
}

#[test]
fn ignores_invalid_page_size() {
    assert!(
        normalize(
            SizeMm::new(0.0, 100.0),
            &[TextMatchRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }]
        )
        .is_empty()
    );
}
