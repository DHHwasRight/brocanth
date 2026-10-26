//! What an operation turned into, before any server's types.
//!
//! [`flow`](crate::flow) decides, [`status`](crate::status) is the table, and
//! this module writes the bytes. It names no `http` type and no page type, so
//! the two surfaces this crate offers render through one code path:
//!
//! * [`routes`](crate::routes) turns an [`Answer`] into an `http::Response`
//!   for a host that mounts a `RouteSource`.
//! * [`pages`](crate::pages) turns the same [`Answer`] into a generated
//!   project's own `Response`.
//!
//! One envelope writer and one status table, so the two surfaces cannot drift
//! apart. That is the whole reason this module exists: a second copy of the
//! table in a second module is a second place to check, and the argument in
//! [`status`](crate::status) is that a reader must be able to check them all
//! at once.
//!
//! Nothing here needs the core, so it compiles with every feature off.

use brocanth_policy::Value;

use crate::error::Failure;
use crate::json;
use crate::page::{Listing, PageRequest};
use crate::resource::{ColumnOf, Direction, Resource, RowOf, Sort};
use crate::status::{generic_message, outcome, Action};

/// What to send, or that there is nothing to send.
///
/// Errors leave by two different doors, which is the split
/// [`Failure::is_internal`] draws. A refusal is an **answer** and is rendered.
/// A failure of the application's own machinery is [`Answer::Internal`], and
/// the surface hands it to whatever the host gives error hooks.
#[derive(Debug)]
pub enum Answer {
    /// A JSON body, its status, and a `Location` for a create.
    Json {
        /// The status the table gave it.
        status: u16,
        /// The body, which is always UTF-8 JSON.
        body: Vec<u8>,
        /// The new row's URL, for a `201`.
        location: Option<String>,
    },

    /// A status and no body at all.
    ///
    /// `204`, where a `content-type` would describe nothing.
    Empty {
        /// The status.
        status: u16,
    },

    /// Not an answer: the application's machinery failed.
    Internal(Failure),
}

/// A status and nothing else.
pub fn empty(status: u16) -> Answer {
    Answer::Empty { status }
}

/// `{"page":1,"perPage":30,"totalItems":2,"totalPages":1,"items":[…]}`.
///
/// `totalItems` and `totalPages` are **absent** when the client passed
/// `skipTotal`, rather than present and wrong. See [`crate::page`].
pub fn envelope<R: Resource>(resource: &R, listing: &Listing<RowOf<R>>) -> Answer {
    let mut body = Vec::with_capacity(64 + listing.items.len() * 64);
    body.push(b'{');
    json::push_key(&mut body, "page");
    json::push_u64(&mut body, listing.page);
    body.push(b',');
    json::push_key(&mut body, "perPage");
    json::push_u64(&mut body, listing.per_page);
    if let Some(total) = listing.total.count() {
        body.push(b',');
        json::push_key(&mut body, "totalItems");
        json::push_u64(&mut body, total);
        if let Some(pages) = listing.total.pages(listing.per_page) {
            body.push(b',');
            json::push_key(&mut body, "totalPages");
            json::push_u64(&mut body, pages);
        }
    }
    body.push(b',');
    json::push_key(&mut body, "items");
    body.push(b'[');
    for (index, row) in listing.items.iter().enumerate() {
        if index > 0 {
            body.push(b',');
        }
        if let Err(failure) = resource.encode_row(row, &mut body) {
            return refuse(Action::List, failure);
        }
    }
    body.extend_from_slice(b"]}");
    Answer::Json {
        status: 200,
        body,
        location: None,
    }
}

/// One row, encoded by the resource that owns it.
pub fn one_row<R: Resource>(
    resource: &R,
    row: &RowOf<R>,
    status: u16,
    location: Option<String>,
) -> Answer {
    let mut body = Vec::with_capacity(128);
    if let Err(failure) = resource.encode_row(row, &mut body) {
        return refuse(Action::View, failure);
    }
    Answer::Json {
        status,
        body,
        location,
    }
}

/// A refusal, rendered; or an internal failure, passed on.
pub fn refuse(action: Action, failure: Failure) -> Answer {
    let answer = outcome(action, &failure);
    if answer.internal {
        return Answer::Internal(failure);
    }
    // A list that nothing matches is an empty page, not an error body: an
    // error body here would be the oracle the 200 exists to avoid.
    if action == Action::List && answer.status == 200 {
        return Answer::Json {
            status: 200,
            body: b"{\"page\":1,\"perPage\":0,\"totalItems\":0,\"totalPages\":0,\"items\":[]}"
                .to_vec(),
            location: None,
        };
    }
    let message = if answer.explain {
        failure.to_string()
    } else {
        generic_message(answer.status).to_owned()
    };
    Answer::Json {
        status: answer.status,
        body: json::error_body(answer.status, &message),
        location: None,
    }
}

/// A key, as a `Location` header spells it.
pub fn display(id: &Value) -> String {
    match id {
        Value::Text(text) => text.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The query string
// ---------------------------------------------------------------------------

/// `page`, `perPage` and `skipTotal`.
///
/// A number that is not a number is a `400`. Clamping it to a default would
/// serve page one to somebody who asked for page `two`, which is the kind of
/// quiet repair that makes a paginator untrustworthy.
///
/// # Errors
///
/// [`Failure::BadRequest`] for a value that is not a number.
pub fn page_request(query: Option<&str>) -> Result<PageRequest, Failure> {
    let mut request = PageRequest::new();
    if let Some(raw) = param(query, "page") {
        request.page = raw
            .parse()
            .map_err(|_| Failure::bad_request(format!("`page` is not a number: `{raw}`")))?;
    }
    if let Some(raw) = param(query, "perPage") {
        request.per_page =
            Some(raw.parse().map_err(|_| {
                Failure::bad_request(format!("`perPage` is not a number: `{raw}`"))
            })?);
    }
    if let Some(raw) = param(query, "skipTotal") {
        request.skip_total = matches!(raw, "" | "1" | "true");
    }
    Ok(request)
}

/// `sort=-created_at`, mapped through [`Resource::sort_column`].
///
/// # Errors
///
/// [`Failure::BadRequest`] for a name this resource does not map. Ignoring it
/// would serve a different order than the one that was asked for, which is the
/// paginator lying again.
pub fn sort_from<R: Resource>(
    resource: &R,
    query: Option<&str>,
) -> Result<Option<Sort<ColumnOf<R>>>, Failure> {
    let Some(raw) = param(query, "sort") else {
        return Ok(None);
    };
    let (direction, name) = match raw.strip_prefix('-') {
        Some(rest) => (Direction::Descending, rest),
        None => (Direction::Ascending, raw.trim_start_matches('+')),
    };
    match resource.sort_column(name) {
        Some(column) => Ok(Some(Sort { column, direction })),
        None => Err(Failure::bad_request(format!(
            "`{name}` is not a sortable column of this resource"
        ))),
    }
}

/// One query-string value, read literally.
///
/// Values are numbers, a flag and a column name, none of which needs
/// percent-decoding, and a decoder here would be a parser this crate does not
/// need.
pub fn param<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    query?
        .split('&')
        .find_map(|pair| match pair.split_once('=') {
            Some((key, value)) if key == name => Some(value),
            None if pair == name => Some(""),
            _ => None,
        })
}
