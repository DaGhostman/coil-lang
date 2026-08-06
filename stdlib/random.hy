// Crypto-backed RNG helpers (userland). Requires virtual `crypto` (default Cargo feature).
use crypto::{random_u64, random_bytes};

/// Uniform `u64` from the host CSPRNG.
fn u64() -> Result<int, CryptoError> {
    return random_u64()?;
}

/// Uniform float in `[0.0, 1.0)`.
fn float() -> Result<float, CryptoError> {
    let x = random_u64()?;
    let m = 9007199254740992;
    let r = x % m;
    let rf = r as float;
    if rf < 0.0 {
        r = 0 - r;
    }
    return (r as float) / (m as float);
}

/// Uniform int in `[lo, hi)` ; empty range → `lo`.
fn range(int lo, int hi) -> Result<int, CryptoError> {
    if hi <= lo {
        return lo;
    }
    let span = hi - lo;
    let x = random_u64()?;
    let xf = x as float;
    if xf < 0.0 {
        x = 0 - x;
    }
    return lo + (x % span);
}

/// Fill a new buffer of `n` random bytes (`n <= 0` → empty).
fn bytes(int n) -> Result<Vec<byte>, CryptoError> {
    if n <= 0 {
        let empty: Vec<byte> = Vec::new();
        return empty;
    }
    return random_bytes(n)?;
}
