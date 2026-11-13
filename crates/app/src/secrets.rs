use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE_NAME: &str = "it.pac-sentinel.portfolio";

pub fn store_secret(key_name: &str, secret_value: &str) -> Result<()> {
    let entry = Entry::new(SERVICE_NAME, key_name)
        .with_context(|| format!("Impossibile inizializzare voce keyring per {key_name}"))?;
    entry
        .set_password(secret_value)
        .with_context(|| format!("Impossibile salvare segreto {key_name} nel keyring"))?;
    Ok(())
}

pub fn load_secret(key_name: &str) -> Option<String> {
    let entry = Entry::new(SERVICE_NAME, key_name).ok()?;
    entry.get_password().ok()
}