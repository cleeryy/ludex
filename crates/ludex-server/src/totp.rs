//! One-time passwords for local accounts (#166).
//!
//! TOTP (RFC 6238) with the RFC 4226 defaults every authenticator app speaks:
//! HMAC-SHA1, six digits, thirty second steps. The SHA-1 and HMAC code is local
//! on purpose — it keeps the dependency tree small and the implementation is
//! checked against the published RFC test vectors below.
//!
//! Recovery codes are generated here too. Only their SHA-256 hex digest is
//! stored; a database dump cannot be turned back into working codes.

use rand::RngCore;
use sha2::{Digest, Sha256};

/// Digits in a generated TOTP code.
pub const DIGITS: u32 = 6;
/// Length of a TOTP time step, in seconds.
pub const PERIOD: i64 = 30;
/// Entropy of a generated TOTP secret, in bytes (160 bits, RFC 4226 §4).
pub const SECRET_BYTES: usize = 20;
/// How many recovery codes are issued at once.
pub const RECOVERY_CODE_COUNT: usize = 10;

/// Base32 alphabet of RFC 4648 (no padding, no ambiguous characters beyond the
/// RFC set: authenticator apps expect the RFC alphabet).
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Recovery-code alphabet: no `0`/`O`, no `1`/`I`, so codes read cleanly.
const RECOVERY_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Encodes bytes as RFC 4648 base32 without `=` padding (otpauth form).
#[must_use]
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    for chunk in bytes.chunks(5) {
        let mut buffer = [0u8; 5];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let value = u64::from(buffer[0]) << 32
            | u64::from(buffer[1]) << 24
            | u64::from(buffer[2]) << 16
            | u64::from(buffer[3]) << 8
            | u64::from(buffer[4]);
        let symbols = (chunk.len() * 8).div_ceil(5);
        for index in 0..symbols {
            let shift = 35 - index * 5;
            let symbol = (value >> shift) & 0x1f;
            out.push(char::from(BASE32[symbol as usize]));
        }
    }
    out
}

/// Decodes RFC 4648 base32, tolerating lowercase, spaces, dashes and padding.
#[must_use]
pub fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut values = Vec::with_capacity(input.len());
    for character in input.chars() {
        match character {
            ' ' | '-' | '=' | '\t' => continue,
            'a'..='z' => values.push(u8::try_from(character as u32 - 'a' as u32).ok()?),
            'A'..='Z' => values.push(u8::try_from(character as u32 - 'A' as u32).ok()?),
            '2'..='7' => values.push(u8::try_from(character as u32 - '2' as u32 + 26).ok()?),
            _ => return None,
        }
    }
    if values.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(values.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for value in values {
        buffer = (buffer << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// Generates a fresh base32 TOTP secret.
#[must_use]
pub fn generate_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    base32_encode(&bytes)
}

/// Computes the HOTP value for `counter` (RFC 4226).
#[must_use]
pub fn hotp(secret: &[u8], counter: u64) -> u32 {
    let mac = hmac_sha1(secret, &counter.to_be_bytes());
    let offset = usize::from(mac[19] & 0x0f);
    let binary = u32::from(mac[offset] & 0x7f) << 24
        | u32::from(mac[offset + 1]) << 16
        | u32::from(mac[offset + 2]) << 8
        | u32::from(mac[offset + 3]);
    binary % 10u32.pow(DIGITS)
}

/// Time step a Unix timestamp falls in.
#[must_use]
pub fn time_step(unix: i64) -> i64 {
    unix.div_euclid(PERIOD)
}

/// Computes the TOTP code for a base32 secret at a Unix timestamp.
#[must_use]
pub fn totp_at(secret_b32: &str, unix: i64) -> Option<String> {
    let secret = base32_decode(secret_b32)?;
    let step = time_step(unix);
    let counter = u64::try_from(step).ok()?;
    Some(format!("{:06}", hotp(&secret, counter)))
}

/// Verifies a submitted code, allowing `window` steps of clock drift.
///
/// Returns the accepted time step, which callers persist to reject a replay of
/// the same code inside its window.
#[must_use]
pub fn verify(secret_b32: &str, code: &str, unix: i64, window: i64) -> Option<i64> {
    let code = normalize_code(code);
    if code.len() != DIGITS as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let submitted = code.parse::<u32>().ok()?;
    let secret = base32_decode(secret_b32)?;
    let center = time_step(unix);
    for offset in -window..=window {
        let step = center + offset;
        let counter = u64::try_from(step).ok()?;
        let expected = hotp(&secret, counter);
        // Constant-time-ish comparison: the candidate space is tiny, but avoid
        // the obvious early-exit anyway.
        if expected ^ submitted == 0 {
            return Some(step);
        }
    }
    None
}

/// Strips spaces and dashes from a submitted six-digit code.
#[must_use]
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .collect()
}

/// `otpauth://` URI an authenticator app scans, and the string a QR encoder
/// would render. Contains the secret: show it once, never log it.
#[must_use]
pub fn provisioning_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    let label = format!("{}:{}", issuer.trim(), account.trim());
    format!(
        "otpauth://totp/{}?secret={}&issuer={}&algorithm=SHA1&digits={}&period={}",
        url_encode(&label),
        secret_b32,
        url_encode(issuer.trim()),
        DIGITS,
        PERIOD
    )
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Generates `RECOVERY_CODE_COUNT` single-use recovery codes, formatted
/// `XXXXX-XXXXX`.
#[must_use]
pub fn generate_recovery_codes(count: usize) -> Vec<String> {
    let mut rng = rand::thread_rng();
    (0..count)
        .map(|_| {
            let mut bytes = [0u8; 10];
            rng.fill_bytes(&mut bytes);
            let code: String = bytes
                .iter()
                .map(|byte| {
                    char::from(RECOVERY_ALPHABET[usize::from(*byte) % RECOVERY_ALPHABET.len()])
                })
                .collect();
            format!("{}-{}", &code[..5], &code[5..])
        })
        .collect()
}

/// Normalises a submitted recovery code for hashing.
#[must_use]
pub fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// SHA-256 hex digest, used for invitation tokens, MFA challenges and recovery
/// codes. Never store the plaintext.
#[must_use]
pub fn hash_secret(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// HMAC-SHA1 (RFC 2104) over the SHA-1 implementation below.
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        key_block[..20].copy_from_slice(&sha1(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    inner.extend(key_block.iter().map(|byte| byte ^ 0x36));
    inner.extend_from_slice(message);
    let inner_hash = sha1(&inner);

    let mut outer = Vec::with_capacity(BLOCK + 20);
    outer.extend(key_block.iter().map(|byte| byte ^ 0x5c));
    outer.extend_from_slice(&inner_hash);
    sha1(&outer)
}

/// SHA-1 (FIPS 180-4). Only used for HMAC inside TOTP, never for passwords.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);

    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let (blocks, _) = message.as_chunks::<64>();
    for block in blocks {
        let mut words = [0u32; 80];
        for (index, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            words[index] = u32::from_be_bytes(*chunk);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (index, value) in h.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_fips_vectors() {
        assert_eq!(
            hex::encode(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex::encode(sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex::encode(sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn hotp_matches_rfc4226_vectors() {
        let secret = b"12345678901234567890";
        let expected = [
            "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583",
            "399871", "520489",
        ];
        for (counter, code) in expected.iter().enumerate() {
            assert_eq!(
                format!("{:06}", hotp(secret, counter as u64)),
                *code,
                "counter {counter}"
            );
        }
    }

    #[test]
    fn totp_matches_rfc6238_vectors() {
        let secret = base32_encode(b"12345678901234567890");
        assert_eq!(totp_at(&secret, 59).as_deref(), Some("287082"));
        assert_eq!(totp_at(&secret, 1_111_111_109).as_deref(), Some("081804"));
        assert_eq!(totp_at(&secret, 1_234_567_890).as_deref(), Some("005924"));
        assert_eq!(totp_at(&secret, 2_000_000_000).as_deref(), Some("279037"));
    }

    #[test]
    fn verification_accepts_the_previous_step_and_rejects_wrong_codes() {
        let secret = base32_encode(b"12345678901234567890");
        let code = totp_at(&secret, 1_000_000_000).expect("code");
        assert_eq!(verify(&secret, &code, 1_000_000_029, 1), Some(33_333_333));
        assert_eq!(verify(&secret, &code, 1_000_000_064, 1), None);
        assert_eq!(verify(&secret, "12345", 1_000_000_000, 1), None);
        assert_eq!(verify(&secret, "abcdef", 1_000_000_000, 1), None);
        // Spaces and dashes are ignored before the digit check.
        assert_eq!(normalize_code("123 456"), "123456");
        assert_eq!(normalize_code("12-34-56"), "123456");
    }

    #[test]
    fn base32_roundtrips_and_tolerates_common_input() {
        for length in 1..40usize {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 7) as u8).collect();
            let encoded = base32_encode(&bytes);
            assert_eq!(base32_decode(&encoded).as_deref(), Some(bytes.as_slice()));
            assert_eq!(
                base32_decode(&encoded.to_lowercase()).as_deref(),
                Some(bytes.as_slice())
            );
        }
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert!(base32_decode("!!!!").is_none());
        assert!(base32_decode("").is_none());
    }

    #[test]
    fn recovery_codes_are_unique_and_normalise() {
        let codes = generate_recovery_codes(RECOVERY_CODE_COUNT);
        assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
        for code in &codes {
            assert_eq!(code.len(), 11);
            assert_eq!(
                code.chars().filter(|character| *character == '-').count(),
                1
            );
            assert_eq!(normalize_recovery_code(code).len(), 10);
            assert!(!normalize_recovery_code(code).contains('-'));
        }
    }

    #[test]
    fn provisioning_uri_carries_the_standard_parameters() {
        let uri = provisioning_uri("Ludex", "mara", "JBSWY3DPEHPK3PXP");
        assert!(uri.starts_with("otpauth://totp/Ludex%3Amara?"));
        assert!(uri.contains("secret=JBSWY3DPEHPK3PXP"));
        assert!(uri.contains("issuer=Ludex"));
        assert!(uri.contains("digits=6"));
        assert!(uri.contains("period=30"));
    }

    #[test]
    fn secret_hashes_are_stable_and_never_the_plaintext() {
        let token = "invite-token";
        let hash = hash_secret(token);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, hash_secret(token));
        assert_ne!(hash, token);
    }
}
