use super::*;

#[test]
fn test_find_coco_home_default() {
    let home = find_coco_home();
    assert!(home.ends_with(COCO_CONFIG_DIR_NAME));
}

#[test]
fn test_coco_config_dir_env_constant() {
    assert_eq!(COCO_CONFIG_DIR_ENV, "COCO_CONFIG_DIR");
}
