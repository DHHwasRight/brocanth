//! `input` -- what `brocanth create` writes when somebody asks for a body
//! that arrives as a struct.
//!
//! The module is here rather than in the CLI because everything it writes
//! is a statement about this crate and about `zod`: that an `Entrance` is
//! built from three parts of a request, that a refused body is 422 and an
//! unreadable one is 400 or 415, and that `Faults` is every fault rather
//! than the first. A copy of that in the CLI rots the first time a name
//! here changes, and it rots in silence.
//!
//! Compiled only under the `scaffold` feature, which is off.
//!
//! # Why this is not `validation`
//!
//! `brocanth-schema` answers "is this value, which I hold, valid?" and
//! mints a `Valid<T>` that proves it. It never touches the request, and
//! before this module there was nothing in a generated project that could
//! make the `T` it wants: `Ctx` carried a verb, a path and the route
//! parameters, and the body went nowhere.
//!
//! This module is the other half. Both may be chosen; neither needs the
//! other.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The entrance, as a choice on the `brocanth create` menu.
pub struct Entry;

impl Module for Entry {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "input",
            "a form or a JSON body, read into a struct, with every fault at once",
        )
        .asked_in("doors", "What arrives: a form or a JSON body, as a struct")
        .krate(Dep::brocanth("brocanth-input"))
        .krate(
            Dep::git("zod", "https://github.com/DHHwasRight/zod.git", "main")
                .note("the rules a field carries, and `#[derive(Entry)]`"),
        )
        .absent(
            "brocanth-input",
            "no entrance; `handle_post` reads `ctx.body` and parses it by hand",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // Into the pages and nowhere else. There is no plugin and no
        // context item: reading a body needs nothing started at boot, and
        // a plugin that only held a function would be a registration a
        // reader has to follow for no gain.
        s.at(Slot::PagesItems).block(items(project));
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The `impl Ctx` the handlers share.
fn items(project: &Project) -> String {
    format!(
        "{HEAD}{}",
        if project.has("rsx") {
            FORM_REFUSAL
        } else {
            API_REFUSAL
        }
    )
}

const HEAD: &str = r##"
// -- the entrance ----------------------------------------------------------
//
// Written because this project chose `input`. A body arrives as bytes and
// leaves as a struct, or as every reason it could not.

/// A body the framework reads and checks before the handler runs.
///
/// `pages!` registers a handler through this one when its signature takes
/// a second parameter, so the page cannot forget to check and cannot
/// check twice.
///
/// `allow(dead_code)`: nothing references it until a page declares a body
/// or a `Json<T>` answer, and a brand-new project has neither. It is here
/// for the first one that does, like the request readers on `Ctx`.
#[allow(dead_code)]
pub fn page_with_body<H, B, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx, B) -> Result<T, Error> + Send + Sync + 'static,
    B: zod::Entry,
    T: IntoResponse,
{
    Box::new(move |ctx| {
        let body = ctx.body_as::<B>()?;
        handler(ctx, body).map(IntoResponse::into_response)
    })
}

/// An answer sent as JSON, and described in the document.
///
/// Wrapping the return type is the opt-in: a page that writes
/// `-> Result<impl IntoResponse, Error>` keeps working and says nothing
/// about its answer, and one that writes `-> Result<Json<Created>, Error>`
/// publishes `Created` as its response schema. The wrapper is the only
/// place that difference is spelled.
///
/// `allow(dead_code)`: nothing references it until a page declares a body
/// or a `Json<T>` answer, and a brand-new project has neither. It is here
/// for the first one that does, like the request readers on `Ctx`.
#[allow(dead_code)]
pub struct Json<T>(pub T);

impl<T: zod::Give> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        Response::new(200, zod::write(&self.0.give()))
            .with_header("content-type", "application/json")
    }
}

/// What came of reading a body.
///
/// Three answers and not two, because they are three different statuses. A
/// body that broke a rule is 422 and the client can fix it. A body nobody
/// could read is 400 or 415, and no amount of fixing the fields will help.
///
/// `allow(dead_code)` on this and on what follows: they are written for
/// the first handler that reads a body, and a brand-new project has none.
/// A scaffold that hands a developer three warnings on their first
/// `cargo clippy` is a scaffold that got it wrong.
#[allow(dead_code)]
pub enum Sent<T> {
    /// It read, and every rule passed.
    Good(T),
    /// It read, and these rules did not. Draw the form again, with
    /// `faults.by_name()` beside the inputs that caused them.
    Faulty(zod::Faults),
    /// Nobody could read it at all.
    Unreadable(brocanth_input::Unreadable),
}

#[allow(dead_code)]
impl Ctx {
    /// The three parts of the request a schema reads: the content type,
    /// the query string and the body.
    pub fn entrance(&self) -> brocanth_input::Entrance<'_> {
        brocanth_input::Entrance::new(self.content_type(), &self.query, &self.body)
    }

    /// Read the body into `T`, keeping every answer apart.
    ///
    /// `T` derives `zod::Entry`, which puts the rules on the struct:
    ///
    /// ```ignore
    /// #[derive(zod::Entry)]
    /// struct Signup {
    ///     #[check(zod::Text::new().min(3).max(16).slug())]
    ///     #[label("Handle")]
    ///     #[says("Pick 3 to 16 lowercase letters, digits or hyphens")]
    ///     handle: String,
    ///
    ///     #[check(zod::Int::new().from(13).to(130))]
    ///     age: i64,
    /// }
    /// ```
    ///
    /// A form and a JSON body both work, and the content type decides how
    /// text is read: a form has no types to lose, so `age=36` is a number
    /// there and is text in JSON.
    pub fn read<T: zod::Entry>(&self) -> Sent<T> {
        match self.entrance().input() {
            Err(why) => Sent::Unreadable(why),
            Ok(input) => match T::parse(input) {
                Ok(value) => Sent::Good(value),
                Err(faults) => Sent::Faulty(faults),
            },
        }
    }

    /// The same, as one `Result` a handler can use `?` on.
    ///
    /// ```ignore
    /// pub fn handle_post(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    ///     let signup: Signup = ctx.body_as()?;
    ///     Ok(Response::empty(303).with_header("location", "/"))
    /// }
    /// ```
    pub fn body_as<T: zod::Entry>(&self) -> Result<T, Error> {
        match self.read() {
            Sent::Good(value) => Ok(value),
            Sent::Faulty(faults) => Err(refused(&faults)),
            Sent::Unreadable(why) => Err(Error::new(why.status(), why.to_string())),
        }
    }

    /// Read the query string into `T`. The same rules, from `?page=2`.
    pub fn query_as<T: zod::Entry>(&self) -> Result<T, Error> {
        T::parse(self.entrance().asked()).map_err(|faults| refused(&faults))
    }
}
"##;

/// For an application that serves HTML. The handler usually draws the form
/// again, so this is the answer for one that has not yet.
const FORM_REFUSAL: &str = r##"
/// Every fault, as the answer this application sends.
///
/// One line per field. An HTML application usually wants the form back with
/// the messages beside the inputs, which only the handler can draw: match
/// on [`Sent::Faulty`] there and render `faults.by_name()`. This is what a
/// handler that has not done that yet sends.
///
/// Not a `From<zod::Faults> for Error`. `Faults` is a `std::error::Error`,
/// so the blanket `From` above already covers it -- and that one answers
/// 500, which is why a handler calls `body_as` rather than `?` on a
/// `Faults`.
#[allow(dead_code)]
pub fn refused(faults: &zod::Faults) -> Error {
    let lines: Vec<String> = faults
        .by_name()
        .into_iter()
        .map(|(name, says)| format!("{name}: {}", says.join("; ")))
        .collect();
    Error::new(422, lines.join("\n"))
}
"##;

/// For an application that answers machines.
const API_REFUSAL: &str = r##"
/// Every fault, as the answer this application sends.
///
/// `application/problem+json` by RFC 9457: a status, a title, and one entry
/// per fault with its path, its code and the numbers it compared. A client
/// can show the message beside the right input without parsing prose.
///
/// Not a `From<zod::Faults> for Error`. `Faults` is a `std::error::Error`,
/// so the blanket `From` above already covers it -- and that one answers
/// 500, which is why a handler calls `body_as` rather than `?` on a
/// `Faults`.
#[allow(dead_code)]
pub fn refused(faults: &zod::Faults) -> Error {
    let problem = zod::Refusal::refuse(&zod::Problem, faults);
    Error::new(problem.status, problem.body).with_header("content-type", problem.content_type)
}
"##;

/// What the generated `AGENTS.md` says.
const AGENTS: &str = "\n\
    ## Reading a body\n\
    \n\
    A handler never parses `ctx.body` itself. It declares a struct with\n\
    `#[derive(zod::Entry)]`, puts the rules on the fields, and calls\n\
    `ctx.body_as()` or `ctx.read()`. The rules are values: a `const`\n\
    shared by two structs is one rule, and `zod::accepts!` makes the\n\
    compiler check a literal against it.\n\
    \n\
    Report every fault, never the first. `Faults` already holds them all\n\
    and `by_name()` groups them per field; a handler that returns after\n\
    the first one makes a person submit the form three times.\n\
    \n\
    Do not write `?` on a `zod::Faults`. The blanket `From` on `Error`\n\
    turns any `std::error::Error` into a 500, and a bad body is a 422.\n\
    `body_as` is the call that gets the status right.\n";
