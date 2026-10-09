#[cfg(target_os = "macos")]
const SERVICE: &str = "dev.ryu.agent-usage";
#[cfg(target_os = "macos")]
const ACCOUNT: &str = "claude.web-cookie";

pub fn read_web_cookie() -> anyhow::Result<Option<String>> {
    #[cfg(target_os = "macos")]
    {
        use security_framework::os::macos::keychain::SecKeychain;
        use security_framework::os::macos::passwords::find_generic_password;

        let _interaction_lock = SecKeychain::disable_user_interaction()?;
        match find_generic_password(None, SERVICE, ACCOUNT) {
            Ok((password, _item)) => Ok(Some(String::from_utf8(password.as_ref().to_vec())?)),
            Err(_) => Ok(None),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

pub fn write_web_cookie(cookie: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use security_framework::os::macos::keychain::SecKeychain;

        SecKeychain::default()?.set_generic_password(SERVICE, ACCOUNT, cookie.as_bytes())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cookie;
        anyhow::bail!("Claude web cookie storage is only available on macOS");
    }
}

pub fn clear_web_cookie() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use security_framework::os::macos::passwords::find_generic_password;

        if let Ok((_password, item)) = find_generic_password(None, SERVICE, ACCOUNT) {
            item.delete();
        }
    }
    Ok(())
}
