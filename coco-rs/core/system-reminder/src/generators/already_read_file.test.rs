use std::path::PathBuf;

use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::ReminderMetadata;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn none_when_paths_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).build();
    assert!(
        AlreadyReadFileGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_silent_attachment_with_metadata() {
    let c = SystemReminderConfig::default();
    let paths = vec![PathBuf::from("/tmp/a.rs"), PathBuf::from("/tmp/b.rs")];
    let ctx = GeneratorContext::builder(&c)
        .already_read_file_paths(paths.clone())
        .build();
    let r = AlreadyReadFileGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::AlreadyReadFile);
    assert!(r.is_effectively_silent());
    match r.metadata {
        Some(ReminderMetadata::AlreadyReadFile(meta)) => assert_eq!(meta.paths, paths),
        other => panic!("expected AlreadyReadFile metadata, got {other:?}"),
    }
}

#[tokio::test]
async fn respects_config_flag() {
    let mut c = SystemReminderConfig::default();
    c.attachments.already_read_file = false;
    assert!(!AlreadyReadFileGenerator.is_enabled(&c));
}
