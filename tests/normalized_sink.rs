use bolt_v2::normalized_sink::spool_root_for_instance;

#[test]
fn builds_live_instance_spool_path() {
    let root = spool_root_for_instance("var/normalized", "instance-123");

    assert_eq!(root, "var/normalized/live/instance-123");
}
