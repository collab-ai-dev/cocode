use super::*;

#[test]
fn serve_defaults_to_sqlite_data_dir_mode() {
    let cli = Cli::parse_from(["coco-hub-server", "serve"]);
    let Command::Serve(args) = cli.command;
    assert!(args.data_dir.is_none());
    assert!(args.memory_base.is_none());
    assert_eq!(args.hub_retention_days, 3);
    assert_eq!(args.hub_retention_max_bytes, 3_221_225_472);
    assert_eq!(args.hub_retention_sweep_interval_secs, 900);
}

#[test]
fn serve_accepts_explicit_data_dir() {
    let cli = Cli::parse_from(["coco-hub-server", "serve", "--data-dir", "/tmp/hub"]);
    let Command::Serve(args) = cli.command;
    assert_eq!(args.data_dir, Some(PathBuf::from("/tmp/hub")));
    assert!(args.memory_base.is_none());
}

#[test]
fn serve_rejects_data_dir_with_memory_base() {
    let err = Cli::try_parse_from([
        "coco-hub-server",
        "serve",
        "--data-dir",
        "/tmp/hub",
        "--memory-base",
        "/tmp/memory",
    ])
    .expect_err("data-dir and memory-base should conflict");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn serve_accepts_retention_overrides() {
    let cli = Cli::parse_from([
        "coco-hub-server",
        "serve",
        "--hub-retention-days",
        "7",
        "--hub-retention-max-bytes",
        "1024",
        "--hub-retention-sweep-interval-secs",
        "60",
    ]);
    let Command::Serve(args) = cli.command;
    assert_eq!(args.hub_retention_days, 7);
    assert_eq!(args.hub_retention_max_bytes, 1024);
    assert_eq!(args.hub_retention_sweep_interval_secs, 60);
}
