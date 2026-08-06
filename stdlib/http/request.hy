// Request builders — layout path for `http::request`.
//
// Canonical implementations live in `http::url` so `http::client` can depend on
// a single sibling module. Prefer:
//   use http::url::{…};   // or use http::client::{…};
// Importing both `http::request` and `http::response` facades that each pull
// from url can hide helpers if names collide — prefer one explicit path.
//
// Re-exported surface (from url):
//   Headers, empty_headers, header_add, headers_count, header_name_at,
//   header_value_at, build_request_head, concat_bytes, parse_url, Url, …
use http::url::{
    Headers,
    Url,
    build_request_head,
    concat_bytes,
    empty_headers,
    header_add,
    header_name_at,
    header_value_at,
    headers_count,
    parse_url,
};
