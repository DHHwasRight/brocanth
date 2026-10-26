//! The entrance: a request becomes the struct a handler asked for, or an
//! answer that says why not.
//!
//! # What was missing
//!
//! A generated project's `Ctx` carried a verb, a path and the route
//! parameters. It did not carry the body, so `handle_post` had nothing to
//! read, and `brocanth-schema` could not help: [`Validation::check`] takes a
//! value that already exists, and nothing in the framework made one.
//!
//! This crate is the missing half. It reads the three parts of a request
//! that say what arrived -- the content type, the query string and the body
//! -- and hands back a [`zod::Input`], which a struct that derives
//! `zod::Entry` reads itself.
//!
//! ```
//! use brocanth_input::Entrance;
//! use zod::{Entry, Int, Text};
//!
//! #[derive(Entry, Debug)]
//! struct Signup {
//!     #[check(Text::new().min(3).max(16).slug())]
//!     handle: String,
//!     #[check(Int::new().from(13).to(130))]
//!     age: i64,
//! }
//!
//! let at = Entrance::new(
//!     Some("application/x-www-form-urlencoded"),
//!     "",
//!     b"handle=ada-lovelace&age=36",
//! );
//!
//! let signup: Signup = Signup::parse(at.input().unwrap()).unwrap();
//! assert_eq!(signup.age, 36);
//! ```
//!
//! The same struct reads a JSON body, because the content type picks the
//! shape and the shape decides whether `"36"` may become a number. A form
//! has no types to lose; a JSON body that sends text for a number has a bug
//! this will not hide.
//!
//! # An upload
//!
//! A `multipart/form-data` body is the third carrier, and it is the one that
//! carries two kinds of thing at once. The same struct reads its text
//! fields, under the same rules a urlencoded form gets, and the files arrive
//! beside the struct rather than inside it:
//!
//! ```ignore
//! let signup: Signup = Signup::parse(at.input()?)?;   // the text fields
//! let photo = at.multipart()?.file("photo");          // the bytes
//! ```
//!
//! A file goes on to `brocanth-storage`, which reads the real media type off
//! the bytes and treats the client's claim as a claim. Every cap a multipart
//! body is read under is in [`multipart::Limits`], and a body over one of
//! them is a 413.
//!
//! # Where this sits beside `brocanth-schema`
//!
//! They answer different questions and they do not overlap.
//!
//! | | `brocanth-schema` | this crate |
//! |---|---|---|
//! | question | is this value, which I hold, valid? | what arrived, and is it a `T`? |
//! | input | a `T` that already exists | bytes |
//! | gives | `Valid<T>`, a typed witness | a `T`, or every fault |
//! | reads | nothing from the request | the content type, the query, the body |
//!
//! A project may use both: this crate to get the struct, `Valid<T>` to carry
//! the proof onward. It may also use either alone.
//!
//! [`Validation::check`]: https://docs.rs/brocanth-schema

#![deny(missing_docs)]

use std::fmt;

pub use described::Described;
pub use multipart::{FilePart, Limits, Multipart};
pub use zod;

use zod::{Faults, Input, Refusal, Value};

pub mod described;
pub mod multipart;

#[cfg(feature = "scaffold")]
pub mod scaffold;

/// Everything the entrance reads out of a request.
///
/// Three borrowed parts and no framework type, so a handler, a hook, a test
/// and a command-line tool all build one the same way.
#[derive(Clone, Copy, Debug)]
pub struct Entrance<'r> {
    content_type: Option<&'r str>,
    query: &'r str,
    body: &'r [u8],
    limits: Limits,
}

/// What the body is written in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Carrier<'r> {
    /// `application/x-www-form-urlencoded`.
    Form,
    /// `application/json`, or a type that ends in `+json`.
    Json,
    /// `multipart/form-data`: text fields and files in one body.
    Multipart,
    /// A type this crate does not read.
    Other(&'r str),
    /// No `content-type` header at all.
    Unsaid,
}

impl<'r> Entrance<'r> {
    /// Build one from the parts of a request, under [`Limits::DEFAULT`].
    pub const fn new(
        content_type: Option<&'r str>,
        query: &'r str,
        body: &'r [u8],
    ) -> Entrance<'r> {
        Entrance {
            content_type,
            query,
            body,
            limits: Limits::DEFAULT,
        }
    }

    /// The same entrance, under caps the caller chose.
    ///
    /// Only multipart reads them. A urlencoded or JSON body is capped by the
    /// server, which refuses it before this crate sees a byte.
    pub const fn with_limits(self, limits: Limits) -> Entrance<'r> {
        Entrance { limits, ..self }
    }

    /// The caps this entrance reads a multipart body under.
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// What the body is written in.
    pub fn carrier(&self) -> Carrier<'r> {
        let Some(said) = self.content_type else {
            return Carrier::Unsaid;
        };
        // The parameters after the first `;` are the charset and the
        // boundary. Neither changes which reader applies.
        let kind = said.split(';').next().unwrap_or("").trim();
        if kind.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
            Carrier::Form
        } else if kind.eq_ignore_ascii_case("application/json")
            || kind.to_ascii_lowercase().ends_with("+json")
        {
            Carrier::Json
        } else if kind.eq_ignore_ascii_case("multipart/form-data") {
            Carrier::Multipart
        } else {
            Carrier::Other(said)
        }
    }

    /// The boundary the content type names, for a multipart body.
    pub fn boundary(&self) -> Option<&'r str> {
        multipart::boundary(self.content_type?)
    }

    /// Read a `multipart/form-data` body: the text fields and the files.
    ///
    /// Whatever the content type says, as long as it names a boundary --
    /// the same deliberate call [`Entrance::as_form`] is, for the same
    /// reason.
    ///
    /// # Errors
    ///
    /// [`Unreadable::Unsaid`] when no content type arrived, and
    /// [`Unreadable::Multipart`] for a body this crate cannot follow or one
    /// over a cap. A cap answers 413 and the rest answer 400.
    pub fn multipart(&self) -> Result<Multipart, Unreadable> {
        let said = self.content_type.ok_or(Unreadable::Unsaid)?;
        let boundary =
            multipart::boundary(said).ok_or(Unreadable::Multipart(multipart::Fault::NoBoundary))?;
        Multipart::parse_with(self.body, boundary, self.limits).map_err(Unreadable::Multipart)
    }

    /// Read the body into input a schema can take.
    ///
    /// An empty body is an empty object, not an error. Every required field
    /// then reports itself missing, which is a 422 a person can act on
    /// rather than a 400 that says only that something was wrong.
    ///
    /// An upload gives its text fields here and its files to
    /// [`Entrance::multipart`], because a schema has no rule that reads
    /// bytes.
    ///
    /// # Errors
    ///
    /// A body this crate cannot read: not UTF-8, not valid JSON, an upload
    /// it cannot follow or one over a cap, or a content type it has no
    /// reader for. A body with no content type at all is refused for the
    /// same reason `ContentTypes` refuses one -- the alternative is to
    /// guess.
    pub fn input(&self) -> Result<Input, Unreadable> {
        if self.body.is_empty() {
            return Ok(Input::flat(Value::Map(Vec::new())));
        }
        match self.carrier() {
            Carrier::Form => Ok(Input::form(self.text()?)),
            Carrier::Json => {
                let parsed: serde_json::Value = serde_json::from_slice(self.body)
                    .map_err(|e| Unreadable::BadJson(e.to_string()))?;
                Ok(Input::json(parsed))
            }
            Carrier::Multipart => {
                // Only the text fields. A handler that wants the files calls
                // `multipart()`, which is the one call that holds bytes --
                // this one walks them, counts them against their cap and
                // copies none of them.
                let said = self.content_type.unwrap_or_default();
                let boundary = multipart::boundary(said)
                    .ok_or(Unreadable::Multipart(multipart::Fault::NoBoundary))?;
                let sent = Multipart::fields_only(self.body, boundary, self.limits)
                    .map_err(Unreadable::Multipart)?;
                Ok(sent.input())
            }
            Carrier::Other(kind) => Err(Unreadable::NoReader(kind.to_string())),
            Carrier::Unsaid => Err(Unreadable::Unsaid),
        }
    }

    /// Read the body as a form, whatever it says it is.
    ///
    /// For a client that posts a form without the header, which is a thing
    /// clients do. The handler asked for it, so this crate does not guess.
    ///
    /// # Errors
    ///
    /// A body that is not UTF-8.
    pub fn as_form(&self) -> Result<Input, Unreadable> {
        Ok(Input::form(self.text()?))
    }

    /// Read the query string, which is a form by another name.
    pub fn asked(&self) -> Input {
        Input::query(self.query)
    }

    /// The body as text.
    ///
    /// # Errors
    ///
    /// A body that is not UTF-8.
    pub fn text(&self) -> Result<&'r str, Unreadable> {
        core::str::from_utf8(self.body).map_err(|_| Unreadable::NotText)
    }

    /// The body as it arrived.
    pub const fn bytes(&self) -> &'r [u8] {
        self.body
    }
}

/// A body the entrance cannot read at all.
///
/// Separate from [`Faults`], and the difference is the status. A fault is a
/// body that arrived and said the wrong thing, which is 422. This is a body
/// that did not arrive in a form anybody can read, which is 400 or 415.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unreadable {
    /// The bytes are not UTF-8.
    NotText,
    /// The body is not valid JSON.
    BadJson(String),
    /// No reader for that content type.
    NoReader(String),
    /// A body arrived with no content type.
    Unsaid,
    /// A `multipart/form-data` body this crate could not follow, or one over
    /// a cap.
    Multipart(multipart::Fault),
}

impl Unreadable {
    /// The status this deserves.
    ///
    /// 415 when the type is the problem and the client can pick another
    /// one; 400 when the bytes are the problem and no type would help; 413
    /// when the body is over a cap, which only a multipart body can be.
    pub const fn status(&self) -> u16 {
        match self {
            Unreadable::NoReader(_) | Unreadable::Unsaid => 415,
            Unreadable::NotText | Unreadable::BadJson(_) => 400,
            Unreadable::Multipart(fault) => fault.status(),
        }
    }
}

impl fmt::Display for Unreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unreadable::NotText => f.write_str("the body is not UTF-8"),
            Unreadable::BadJson(why) => write!(f, "the body is not valid JSON: {why}"),
            Unreadable::NoReader(kind) => {
                write!(f, "no reader for `{kind}`; send a form or JSON")
            }
            Unreadable::Unsaid => {
                f.write_str("a body arrived with no content-type; send a form or JSON")
            }
            Unreadable::Multipart(fault) => write!(f, "the upload could not be read: {fault}"),
        }
    }
}

impl std::error::Error for Unreadable {}

/// Every fault, as an answer the server can write.
///
/// `application/problem+json` by RFC 9457, status 422, one entry per fault
/// with its path, its code and the numbers it compared. It is
/// [`zod::Problem`] with the body put in a [`brocanth_server::Response`].
#[derive(Clone, Copy, Default, Debug)]
pub struct Answer;

impl Refusal for Answer {
    type Out = brocanth_server::Response;

    fn refuse(&self, faults: &Faults) -> brocanth_server::Response {
        let problem = zod::Problem.refuse(faults);
        brocanth_server::Response::new(problem.status, problem.content_type, problem.body)
    }
}

/// A body nobody could read, as an answer the server can write.
pub fn unreadable(why: &Unreadable) -> brocanth_server::Response {
    brocanth_server::Response::text(why.status(), why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zod::{Entry, Int, Schema, Text};

    #[derive(Entry, Debug, PartialEq)]
    struct Signup {
        #[check(Text::new().min(3).max(16).slug())]
        handle: String,
        #[check(Int::new().from(13).to(130))]
        age: i64,
    }

    const FORM: &str = "application/x-www-form-urlencoded";

    #[test]
    fn a_form_becomes_the_struct_the_handler_asked_for() {
        let at = Entrance::new(Some(FORM), "", b"handle=ada-lovelace&age=36");
        let signup = Signup::parse(at.input().unwrap()).unwrap();
        assert_eq!(
            signup,
            Signup {
                handle: "ada-lovelace".into(),
                age: 36
            }
        );
    }

    #[test]
    fn a_json_body_becomes_the_same_struct() {
        let at = Entrance::new(
            Some("application/json"),
            "",
            br#"{"handle": "ada-lovelace", "age": 36}"#,
        );
        let signup = Signup::parse(at.input().unwrap()).unwrap();
        assert_eq!(signup.age, 36);
    }

    #[test]
    fn the_carrier_ignores_the_charset_that_follows_it() {
        let at = Entrance::new(Some("application/json; charset=utf-8"), "", b"{}");
        assert_eq!(at.carrier(), Carrier::Json);
    }

    #[test]
    fn a_vendor_type_that_ends_in_json_is_json() {
        let at = Entrance::new(Some("application/vnd.api+json"), "", b"{}");
        assert_eq!(at.carrier(), Carrier::Json);
    }

    #[test]
    fn a_form_converts_text_and_a_json_body_does_not() {
        let form = Entrance::new(Some(FORM), "", b"handle=ada-lovelace&age=36");
        assert!(Signup::parse(form.input().unwrap()).is_ok());

        let json = Entrance::new(
            Some("application/json"),
            "",
            br#"{"handle": "ada-lovelace", "age": "36"}"#,
        );
        let faults = Signup::parse(json.input().unwrap()).unwrap_err();
        assert_eq!(faults.all()[0].flaw.code, zod::Code::WrongType);
    }

    #[test]
    fn an_empty_body_is_an_empty_object_so_every_field_says_it_is_missing() {
        let at = Entrance::new(Some(FORM), "", b"");
        let faults = Signup::parse(at.input().unwrap()).unwrap_err();

        assert_eq!(faults.len(), 2);
        assert!(faults
            .all()
            .iter()
            .all(|f| f.flaw.code == zod::Code::Missing));
    }

    #[test]
    fn a_body_with_no_content_type_is_refused_rather_than_guessed() {
        let at = Entrance::new(None, "", b"handle=ada");
        let why = at.input().unwrap_err();
        assert_eq!(why, Unreadable::Unsaid);
        assert_eq!(why.status(), 415);
    }

    #[test]
    fn a_type_with_no_reader_names_itself() {
        let at = Entrance::new(Some("text/csv"), "", b"a,b");
        let why = at.input().unwrap_err();
        assert_eq!(why.status(), 415);
        assert!(why.to_string().contains("text/csv"));
    }

    #[test]
    fn json_that_does_not_parse_is_four_hundred_not_four_twenty_two() {
        let at = Entrance::new(Some("application/json"), "", b"{\"handle\":");
        let why = at.input().unwrap_err();
        assert_eq!(why.status(), 400);
    }

    #[test]
    fn a_body_that_is_not_utf8_says_so_instead_of_panicking() {
        let at = Entrance::new(Some(FORM), "", &[0xff, 0xfe]);
        assert_eq!(at.input().unwrap_err(), Unreadable::NotText);
    }

    #[test]
    fn a_query_string_reads_like_a_form_because_it_is_one() {
        #[derive(Entry, Debug)]
        struct Page {
            #[check(Int::new().from(1).to(999))]
            page: i64,
            #[check(Text::new().min(1).optional())]
            q: Option<String>,
        }

        let at = Entrance::new(None, "?page=2&q=rust", b"");
        let asked = Page::parse(at.asked()).unwrap();
        assert_eq!(asked.page, 2);
        assert_eq!(asked.q.as_deref(), Some("rust"));
    }

    #[test]
    fn a_form_posted_without_the_header_can_still_be_read_on_purpose() {
        let at = Entrance::new(None, "", b"handle=ada-lovelace&age=36");
        assert!(at.input().is_err());
        assert!(Signup::parse(at.as_form().unwrap()).is_ok());
    }

    #[test]
    fn every_fault_comes_back_as_one_answer_the_server_can_write() {
        let at = Entrance::new(Some(FORM), "", b"handle=A&age=200");
        let answer = Signup::parse(at.input().unwrap()).unwrap_err();
        let sent = Answer.refuse(&answer);

        assert_eq!(sent.status, 422);
        assert_eq!(
            sent.header("content-type"),
            Some("application/problem+json")
        );
    }

    #[test]
    fn a_body_nobody_can_read_answers_with_the_reason_in_plain_text() {
        let sent = unreadable(&Unreadable::NoReader("text/csv".into()));
        assert_eq!(sent.status, 415);
    }

    #[test]
    fn the_query_string_may_carry_the_question_mark_or_not() {
        let with = Entrance::new(None, "?a=1", b"");
        let without = Entrance::new(None, "a=1", b"");
        assert_eq!(with.asked(), without.asked());
    }

    #[test]
    fn a_struct_the_program_built_is_checked_without_parsing_it_again() {
        let held = Signup {
            handle: "ada-lovelace".into(),
            age: 36,
        };
        assert!(held.sound());

        let wrong = Signup {
            handle: "A".into(),
            age: 200,
        };
        assert_eq!(wrong.check().len(), 3);
    }

    // -- multipart -------------------------------------------------------

    const MULTI: &str = "multipart/form-data; boundary=x";

    /// A body with a text field and a file, written the way a browser does.
    fn upload() -> Vec<u8> {
        b"--x\r\n\
          Content-Disposition: form-data; name=\"handle\"\r\n\r\n\
          ada-lovelace\r\n\
          --x\r\n\
          Content-Disposition: form-data; name=\"age\"\r\n\r\n\
          36\r\n\
          --x\r\n\
          Content-Disposition: form-data; name=\"photo\"; filename=\"cat.png\"\r\n\
          Content-Type: image/png\r\n\r\n\
          \x89PNG\r\n\x1a\n\r\n\
          --x--\r\n"
            .to_vec()
    }

    #[test]
    fn a_multipart_body_is_a_carrier_of_its_own() {
        let at = Entrance::new(Some(MULTI), "", b"");
        assert_eq!(at.carrier(), Carrier::Multipart);
        assert_eq!(at.boundary(), Some("x"));

        let cased = Entrance::new(Some("Multipart/Form-Data; boundary=\"a b\""), "", b"");
        assert_eq!(cased.carrier(), Carrier::Multipart);
        assert_eq!(cased.boundary(), Some("a b"));
    }

    #[test]
    fn a_multipart_bodys_text_fields_are_checked_by_the_rules_a_form_gets() {
        let body = upload();
        let at = Entrance::new(Some(MULTI), "", &body);
        let signup = Signup::parse(at.input().unwrap()).unwrap();

        assert_eq!(
            signup,
            Signup {
                handle: "ada-lovelace".into(),
                age: 36
            }
        );
    }

    #[test]
    fn the_same_struct_reads_a_form_a_json_body_and_an_upload() {
        let body = upload();
        let cases: [(&str, &[u8]); 3] = [
            (FORM, b"handle=ada-lovelace&age=36"),
            (
                "application/json",
                br#"{"handle": "ada-lovelace", "age": 36}"#,
            ),
            (MULTI, &body),
        ];
        for (kind, bytes) in cases {
            let at = Entrance::new(Some(kind), "", bytes);
            assert_eq!(
                Signup::parse(at.input().unwrap()).unwrap().age,
                36,
                "{kind}"
            );
        }
    }

    #[test]
    fn a_broken_upload_refuses_the_whole_body_rather_than_half_of_it() {
        let at = Entrance::new(Some(MULTI), "", b"--x\r\nContent-Dispo");
        let why = at.input().unwrap_err();

        assert_eq!(why, Unreadable::Multipart(multipart::Fault::NoHeaderEnd));
        assert_eq!(why.status(), 400);
        assert!(why.to_string().contains("headers have no end"));
    }

    #[test]
    fn a_multipart_type_with_no_boundary_is_four_hundred_and_not_a_guess() {
        let at = Entrance::new(Some("multipart/form-data"), "", b"--x--\r\n");
        let why = at.input().unwrap_err();

        assert_eq!(why, Unreadable::Multipart(multipart::Fault::NoBoundary));
        assert_eq!(why.status(), 400);
    }

    #[test]
    fn the_files_arrive_beside_the_struct_and_not_inside_it() {
        let body = upload();
        let at = Entrance::new(Some(MULTI), "", &body);

        let signup = Signup::parse(at.input().unwrap()).unwrap();
        assert_eq!(signup.handle, "ada-lovelace");

        let sent = at.multipart().unwrap();
        let photo = sent.file("photo").unwrap();
        assert_eq!(photo.filename(), Some("cat.png"));
        assert_eq!(photo.declared_type(), Some("image/png"));
        assert_eq!(photo.bytes(), b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn a_body_over_a_cap_is_four_hundred_and_thirteen_at_the_entrance() {
        let tight = Limits {
            total_bytes: 32,
            ..Limits::DEFAULT
        };
        let body = upload();
        let at = Entrance::new(Some(MULTI), "", &body).with_limits(tight);

        assert_eq!(at.limits().total_bytes, 32);
        for why in [at.input().unwrap_err(), at.multipart().unwrap_err()] {
            assert_eq!(why.status(), 413);
            assert_eq!(unreadable(&why).status, 413);
        }
    }

    #[test]
    fn an_upload_with_no_content_type_at_all_is_refused_before_it_is_parsed() {
        let body = upload();
        let at = Entrance::new(None, "", &body);
        assert_eq!(at.multipart().unwrap_err(), Unreadable::Unsaid);
    }

    #[test]
    fn an_empty_upload_is_an_empty_object_like_every_other_empty_body() {
        let at = Entrance::new(Some(MULTI), "", b"");
        let faults = Signup::parse(at.input().unwrap()).unwrap_err();
        assert_eq!(faults.len(), 2);
    }

    #[test]
    fn the_schema_in_a_field_is_the_one_the_compiler_also_runs() {
        const HANDLE: Text = Text::new().min(3).max(16).slug();
        zod::accepts!(HANDLE, "ada-lovelace");
        assert!(HANDLE.accepts("ada-lovelace"));
    }
}
