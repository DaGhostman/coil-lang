// Request builders — canonical implementations live in `http::url` so
// `http::client` can depend on a single module. Prefer:
//   use http::url::*;   // or use http::client::*;
// Globbing both `http::request` and `http::response` (which each import url)
// has been observed to hide url helpers (multi-glob quirk).
use http::url::*;
