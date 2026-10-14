use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;

pub(crate) fn prevent_attribution() -> Result<()> {
    let path = dirs::home_dir()
        .context("find home directory")?
        .join(".claude/settings.json");
    let mut settings: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?)?
    } else {
        serde_json::json!({})
    };
    settings
        .as_object_mut()
        .context("settings must be a JSON object")?
        .insert("includeCoAuthoredBy".into(), Value::Bool(false));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&settings)?),
    )?;
    println!("Set includeCoAuthoredBy=false in {}", path.display());
    Ok(())
}
