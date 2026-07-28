# `crypto` module

`use crypto::*;` — one-shot and streaming hashes (`sha256`, `init` / `update` / `finalize`), HMAC, `random_bytes`, ChaCha20-Poly1305 and AES-256-GCM, Ed25519 / X25519, Argon2id, constant-time `ct_eq`. Pure Rust (RustCrypto); no OpenSSL. Argon2id uses fixed MVP params (19 MiB memory, 2 iterations, parallelism 1); salts shorter than 16 bytes are zero-padded to 16 — not OWASP-tunable.

---
