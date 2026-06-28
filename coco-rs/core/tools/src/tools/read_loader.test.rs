use std::io::Write;
use std::path::Path;

use super::*;
use crate::tools::read::ReadInput;

#[test]
fn classify_read_path_matches_read_tool_special_cases() {
    assert_eq!(
        classify_read_path(Path::new("/tmp/screen.png")),
        ReadFileKind::SupportedImage {
            media_type: "image/png"
        }
    );
    assert_eq!(
        classify_read_path(Path::new("/tmp/vector.svg")),
        ReadFileKind::PlaceholderImage {
            extension: "svg".to_string()
        }
    );
    assert_eq!(
        classify_read_path(Path::new("/tmp/archive.zip")),
        ReadFileKind::Binary {
            extension: "zip".to_string()
        }
    );
    assert_eq!(
        classify_read_path(Path::new("/tmp/report.pdf")),
        ReadFileKind::Pdf
    );
    assert_eq!(
        classify_read_path(Path::new("/tmp/notebook.ipynb")),
        ReadFileKind::Notebook
    );
    assert_eq!(
        classify_read_path(Path::new("/tmp/main.rs")),
        ReadFileKind::Text
    );
}

#[test]
fn read_text_selection_formats_full_text_and_caches_raw_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("file.txt");
    {
        let mut f = std::fs::File::create(&file).expect("create");
        f.write_all(b"alpha\nbeta\n").expect("write");
    }

    let input = ReadInput {
        file_path: file.display().to_string(),
        offset: None,
        limit: None,
        pages: None,
    };
    let selection = read_text_selection(&input.file_path, &input).expect("read");

    assert_eq!(selection.output, "1\talpha\n2\tbeta\n");
    assert_eq!(selection.cached_content, "alpha\nbeta\n");
    assert_eq!(selection.range, coco_context::FileReadRange::Full);
    assert!(selection.should_record);
}

#[test]
fn changed_file_text_loader_returns_decoded_raw_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("changed.txt");
    std::fs::write(&file, "raw\ncontent\n").expect("write");

    let content = read_full_text_for_changed_file(&file).expect("read");

    assert_eq!(content, "raw\ncontent\n");
}
