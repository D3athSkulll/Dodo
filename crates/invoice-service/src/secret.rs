//! Random secret generation. Used for API-key material and webhook signing
//! secrets — both are high-entropy random strings, so one helper covers both.

use std::fmt::Write;

/// `n_bytes` of CSPRNG output as a lowercase hex string (`2 * n_bytes` chars).
pub fn hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    getrandom::getrandom(&mut buf).expect("system CSPRNG unavailable");

    let mut s = String::with_capacity(n_bytes * 2);
    for b in &buf {
        let _ = write!(s, "{b:02x}");
    }
    s
}
