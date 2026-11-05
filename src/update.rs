use std::time::Duration;

use anyhow::Result;
use self_update::backends::github::Update;
use self_update::cargo_crate_version;

/// リリースの zip を zipsign で署名した鍵の公開鍵。秘密鍵は GitHub Secrets の ZIPSIGN_PRIVATE_KEY。
const VERIFYING_KEY: self_update::VerifyingKey = *include_bytes!("../signing-public.key");

fn updater(timeout: Duration) -> Result<Update> {
    Ok(Update::configure()
        .repo_owner("taaaaaaachu")
        .repo_name("auto_backup")
        .bin_name("auto_backup")
        .current_version(cargo_crate_version!())
        .verifying_keys([VERIFYING_KEY])
        .timeout(timeout)
        .show_output(false)
        .no_confirm(true)
        .build()?)
}

/// 最新のリリースが今の版より新しければ、その版を返す。
pub fn newer_version() -> Result<Option<String>> {
    let releases = updater(Duration::from_secs(5))?.get_latest_release()?;
    if !releases.is_update_available()? {
        return Ok(None);
    }
    Ok(releases.latest().map(|r| r.version().to_string()))
}

/// 最新のリリースに自分自身を置き換える。
pub fn run() -> Result<()> {
    let current = cargo_crate_version!();
    let status = updater(Duration::from_secs(60))?.update()?;
    if status.is_updated() {
        println!("updated {current} -> {}", status.version());
    } else {
        println!("already up to date ({current})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_builds_with_embedded_key() {
        updater(Duration::from_secs(1)).expect("updater config should be valid");
    }
}
