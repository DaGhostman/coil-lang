// Request builders — layout path for `http::request`.
//
// Canonical implementations live in `http::url` so `http::client` can depend on
// a single sibling module. Prefer:
//   use http::url::*;   // or use http::client::*;
// Globbing both `http::request` and `http::response` (which each import url)
// has been observed to hide url helpers (multi-glob quirk).
//
// Re-exported surface (from url):
//   Headers, empty_headers, header_add, headers_count, header_name_at,
//   header_value_at, build_request_head, concat_bytes, parse_url, Url, …
use http::url::*;
