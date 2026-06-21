use super::*;

#[test]
fn test_find_coco_home_default() {
    // When COCO_CONFIG_DIR is not set, should return ~/.coco
    let home = find_coco_home();
    assert!(home.ends_with(DEFAULT_DIR));
}

#[test]
fn test_coco_config_dir_env_constant() {
    assert_eq!(COCO_CONFIG_DIR_ENV, "COCO_CONFIG_DIR");
}
