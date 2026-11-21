//! Validation and generation of hotspot credentials.

use std::fs::File;
use std::io::Read;
use std::ops::RangeInclusive;

const SSID_LEN_BYTES: RangeInclusive<usize> = 1..=32;
const PASSWORD_LEN: RangeInclusive<usize> = 8..=63;
const GENERATED_PASSWORD_LEN: usize = 12;
/// Omits look-alike characters (0/O, 1/l/I) so the password is easy to read and type.
const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn validate_ssid(ssid: &str) -> Result<(), &'static str> {
    if !SSID_LEN_BYTES.contains(&ssid.len()) {
        return Err("SSID must be 1 to 32 bytes long");
    }
    if ssid.chars().any(char::is_control) {
        return Err("SSID must not contain control characters");
    }
    Ok(())
}

/// WPA2 passphrases are 8 to 63 printable ASCII characters.
pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if !password.chars().all(|c| matches!(c, ' '..='~')) {
        return Err("Password must contain printable ASCII characters only");
    }
    if !PASSWORD_LEN.contains(&password.len()) {
        return Err("Password must be 8 to 63 characters long");
    }
    Ok(())
}

/// Returns a random password from the kernel CSPRNG, or `None` if it is unavailable.
pub fn generate_password() -> Option<String> {
    let mut entropy = [0u8; 64];
    File::open("/dev/urandom")
        .and_then(|mut urandom| urandom.read_exact(&mut entropy))
        .ok()?;
    password_from_bytes(&entropy, GENERATED_PASSWORD_LEN)
}

fn password_from_bytes(bytes: &[u8], len: usize) -> Option<String> {
    let alphabet_len = PASSWORD_ALPHABET.len();
    // Rejection sampling: drop bytes above the largest multiple of the
    // alphabet size so every character is equally likely.
    let limit = 256 - (256 % alphabet_len);
    let password: String = bytes
        .iter()
        .map(|&b| usize::from(b))
        .filter(|&b| b < limit)
        .map(|b| char::from(PASSWORD_ALPHABET[b % alphabet_len]))
        .take(len)
        .collect();
    (password.len() == len).then_some(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_length_is_checked_in_ascii_characters() {
        assert!(validate_password("1234567").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password(&"a".repeat(63)).is_ok());
        assert!(validate_password(&"a".repeat(64)).is_err());
        // 8 bytes but only 4 characters, and not ASCII.
        assert!(validate_password("éééé").is_err());
        assert!(validate_password("pass\tword").is_err());
    }

    #[test]
    fn ssid_is_limited_to_32_bytes_without_control_characters() {
        assert!(validate_ssid("").is_err());
        assert!(validate_ssid(&"a".repeat(32)).is_ok());
        assert!(validate_ssid(&"a".repeat(33)).is_err());
        assert!(validate_ssid("Café Wi-Fi").is_ok());
        assert!(validate_ssid("bad\nname").is_err());
    }

    #[test]
    fn generated_password_is_valid_and_uses_the_alphabet() {
        let password = generate_password().expect("/dev/urandom should be readable");
        assert_eq!(password.len(), GENERATED_PASSWORD_LEN);
        assert!(validate_password(&password).is_ok());
        assert!(password.bytes().all(|b| PASSWORD_ALPHABET.contains(&b)));
    }

    #[test]
    fn biased_bytes_are_rejected_and_short_input_yields_none() {
        // 57 characters -> bytes >= 228 are rejected.
        assert_eq!(password_from_bytes(&[255, 0, 230, 1], 2), Some("ab".into()));
        assert_eq!(password_from_bytes(&[255, 0], 2), None);
    }
}
