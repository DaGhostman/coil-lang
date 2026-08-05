// Response parse — layout path for `http::response`.
//
// Canonical implementations live in `http::url` (same single-import reason as
// `request.hy`). Prefer `use http::url::{…};` or `use http::client::{…};`.
//
// Re-exported surface (from url):
//   Response, parse_response, response_status, response_body_len,
//   header_get, header_count, make_response, HttpError, …
use http::url::{
    HttpError,
    Response,
    header_count,
    header_get,
    make_response,
    parse_response,
    response_body_len,
    response_status,
};
