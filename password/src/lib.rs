//! How the password of the machine's person is kept.
//!
//! Not the password: a number worked out from it that the password can't
//! be worked back out of, and that takes long enough to work out that
//! trying every word in a dictionary against a stolen disk takes years
//! instead of seconds. That is PBKDF2 with HMAC-SHA-256 (RFC 8018), the
//! same construction Wi-Fi passwords and disk encryption use, with a salt
//! so that two people with one password don't have one number.
//!
//! What is kept is one line:
//!
//!   pbkdf2-sha256:ROUNDS:SALT:NUMBER      (the last two in hexadecimal)
//!
//! The rounds are in the line so they can be raised for new passwords
//! without old ones ceasing to be right.

use sha2::{Digest, Sha256};

/// How many times the hash is turned for a new password: about a tenth of
/// a second on a laptop, and a second on a netbook's Atom.
pub const ROUNDS: u32 = 60_000;

const NAME: &str = "pbkdf2-sha256";
const SALT: usize = 16;
const BLOCK: usize = 64;
const OUT: usize = 32;

/// The line to keep for `password`, salted from the kernel's randomness.
pub fn keep(password: &str) -> std::io::Result<String> {
    use std::io::Read;
    let mut salt = [0u8; SALT];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut salt)?;
    Ok(keep_with(password, &salt, ROUNDS))
}

/// The line to keep for `password` with a given salt and rounds.
pub fn keep_with(password: &str, salt: &[u8], rounds: u32) -> String {
    let number = derive(password.as_bytes(), salt, rounds);
    format!("{NAME}:{rounds}:{}:{}", hex(salt), hex(&number))
}

/// Whether `password` is the one `kept` was made from. A line that isn't
/// one of these is no password's.
pub fn is(kept: &str, password: &str) -> bool {
    let mut fields = kept.trim().split(':');
    let (Some(name), Some(rounds), Some(salt), Some(number), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let (Ok(rounds), Some(salt), Some(number)) =
        (rounds.parse::<u32>(), unhex(salt), unhex(number))
    else {
        return false;
    };
    if name != NAME || rounds == 0 || number.len() != OUT {
        return false;
    }
    let tried = derive(password.as_bytes(), &salt, rounds);
    // Every byte is looked at whichever differs first, so how long the
    // answer takes says nothing of how nearly right the password was.
    tried
        .iter()
        .zip(&number)
        .fold(0u8, |differing, (a, b)| differing | (a ^ b))
        == 0
}

/// PBKDF2's first block: all this needs, a hash's length being the length
/// wanted.
fn derive(password: &[u8], salt: &[u8], rounds: u32) -> [u8; OUT] {
    let (inner, outer) = pads(password);

    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut turn = hmac(&inner, &outer, &first);
    let mut out = turn;
    for _ in 1..rounds {
        turn = hmac(&inner, &outer, &turn);
        for (o, t) in out.iter_mut().zip(&turn) {
            *o ^= t;
        }
    }
    out
}

/// HMAC's two keyed starts, made once: every round begins from them.
fn pads(key: &[u8]) -> (Sha256, Sha256) {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..OUT].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    (inner, outer)
}

fn hmac(inner: &Sha256, outer: &Sha256, message: &[u8]) -> [u8; OUT] {
    let mut hash = inner.clone();
    hash.update(message);
    let within = hash.finalize();
    let mut hash = outer.clone();
    hash.update(within);
    hash.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7914 §11's test vectors for PBKDF2-HMAC-SHA-256, their first
    /// 32 bytes.
    #[test]
    fn it_is_the_pbkdf2_of_the_rfc() {
        assert_eq!(
            hex(&derive(b"passwd", b"salt", 1)),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
        );
        assert_eq!(
            hex(&derive(b"Password", b"NaCl", 80000)),
            "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56"
        );
    }

    #[test]
    fn longer_passwords_and_salts_than_a_block_s_half() {
        // RFC 6070's longer case, as it is published for SHA-256; and a
        // password longer than a block, which HMAC hashes down first.
        let number = derive(
            b"passwordPASSWORDpassword",
            b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
            4096,
        );
        assert_eq!(
            hex(&number),
            "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1"
        );
        let long = "x".repeat(100);
        assert!(is(&keep_with(&long, b"0123456789abcdef", 10), &long));
    }

    #[test]
    fn only_the_password_it_was_made_from_is_right() {
        let kept = keep_with("saulė 123", b"0123456789abcdef", 100);
        assert!(kept.starts_with("pbkdf2-sha256:100:30313233"));
        assert!(is(&kept, "saulė 123"));
        assert!(!is(&kept, "saule 123"));
        assert!(!is(&kept, ""));
    }

    #[test]
    fn the_same_password_is_kept_differently_each_time() {
        let (a, b) = (keep("one").unwrap(), keep("one").unwrap());
        assert_ne!(a, b);
        assert!(is(&a, "one") && is(&b, "one"));
    }

    #[test]
    fn a_line_that_is_not_one_of_these_is_nobody_s_password() {
        assert!(!is("", ""));
        assert!(!is("plain:1:00:00", ""));
        assert!(!is("pbkdf2-sha256:0:00:00", ""));
        assert!(!is("pbkdf2-sha256:10:zz:00", "x"));
        assert!(!is("pbkdf2-sha256:10:00:00:extra", "x"));
    }
}
