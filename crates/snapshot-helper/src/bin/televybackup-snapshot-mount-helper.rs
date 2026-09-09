#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "televybackup-snapshot-mount-helper {} ({})",
            option_env!("TELEVYBACKUP_BUILD_VERSION")
                .unwrap_or(televybackup_snapshot_access::mount_helper::ROOT_MOUNT_HELPER_VERSION),
            option_env!("TELEVYBACKUP_BUILD_COMMIT").unwrap_or("unknown")
        );
        return Ok(());
    }
    televybackup_snapshot_access::mount_helper::run_server().await?;
    Ok(())
}
