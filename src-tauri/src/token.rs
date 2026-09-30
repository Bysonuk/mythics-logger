//! The app token, kept in the operating system's credential store (Windows
//! Credential Manager; the macOS Keychain later), never in a plain file.

const SERVICE: &str = "gg.mythics.logger";
const USER: &str = "app-token";

fn entry() -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, USER).ok()
}

pub fn load() -> Option<String> {
    entry()?.get_password().ok().filter(|t| !t.is_empty())
}

pub fn save(token: &str) -> Result<(), String> {
    entry()
        .ok_or_else(|| "credential store unavailable".to_string())?
        .set_password(token)
        .map_err(|_| "couldn't save to the credential store".to_string())
}

pub fn clear() {
    if let Some(e) = entry() {
        let _ = e.delete_credential();
    }
}
