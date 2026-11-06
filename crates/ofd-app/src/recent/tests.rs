use super::*;

#[test]
fn round_trips_recent_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("recent.txt");
    let paths = vec![PathBuf::from("/a.ofd"), PathBuf::from("/b.ofd")];
    save_to(&file, &paths, 10);
    assert_eq!(load_from(&file, 10), paths);
}

#[test]
fn moves_existing_path_to_front() {
    let mut paths = vec![
        PathBuf::from("/a.ofd"),
        PathBuf::from("/b.ofd"),
        PathBuf::from("/c.ofd"),
    ];
    push_recent(&mut paths, Path::new("/b.ofd"), 10);
    assert_eq!(
        paths,
        vec![
            PathBuf::from("/b.ofd"),
            PathBuf::from("/a.ofd"),
            PathBuf::from("/c.ofd")
        ]
    );
}

#[test]
fn caps_list_and_keeps_newest() {
    let mut paths = vec![];
    for index in 0..5 {
        push_recent(&mut paths, Path::new(&format!("/{index}.ofd")), 3);
    }
    assert_eq!(paths.len(), 3);
    assert_eq!(paths[0], PathBuf::from("/4.ofd"));
    assert_eq!(paths[2], PathBuf::from("/2.ofd"));
}
