use crate::rpc::RpcError;

#[cfg(target_os = "macos")]
pub fn set_secret(key: &str, value: &str) -> Result<(), RpcError> {
    let entry = keyring::Entry::new("TelevyBackup", key)
        .map_err(|e| RpcError::new("keychain.unavailable", format!("keyring init failed: {e}")))?;
    entry.set_password(value).map_err(|e| {
        RpcError::new(
            "keychain.write_failed",
            format!("keyring write failed: {e}"),
        )
    })?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn get_secret(key: &str) -> Result<Option<String>, RpcError> {
    let entry = keyring::Entry::new("TelevyBackup", key)
        .map_err(|e| RpcError::new("keychain.unavailable", format!("keyring init failed: {e}")))?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(RpcError::new(
            "keychain.unavailable",
            format!("keyring read failed: {e}"),
        )),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_secret(_key: &str, _value: &str) -> Result<(), RpcError> {
    Err(RpcError::new(
        "keychain.unavailable",
        "Keychain secrets are only supported on macOS in this build".to_string(),
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn get_secret(_key: &str) -> Result<Option<String>, RpcError> {
    Ok(None)
}
