fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(status) =
        televybackup_snapshot_access::mount_helper::maybe_authorize_source_path(&args)
    {
        std::process::exit(status);
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "televybackup-snapshot-mount-helper {} ({})",
            option_env!("TELEVYBACKUP_BUILD_VERSION")
                .unwrap_or(televybackup_snapshot_access::mount_helper::ROOT_MOUNT_HELPER_VERSION),
            option_env!("TELEVYBACKUP_BUILD_COMMIT").unwrap_or("unknown")
        );
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(televybackup_snapshot_access::mount_helper::run_server())?;
    Ok(())
}
