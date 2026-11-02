//! The README's page tree, as a project writes it.
//!
//! A generated project writes this file as `src/pages/mod.rs`. This one is
//! smaller: it defines only what `pages!` and `html!` name. `views` is
//! written by hand here, where a project has
//! `pub mod views { rsx::load_dir!("src/pages"); }`, so the test needs no
//! template engine.

brocanth_pages::pages!("tests/readme/src/pages");

pub use brocanth_pages::html;

use brocanth::routing::{BuildError, Router, Verb};

/// What a handler is given.
pub struct Ctx {
    pub params: Vec<(String, String)>,
    /// How many layout layers the client already holds.
    pub held: u32,
}

impl Ctx {
    pub fn require(&self, name: &str) -> Result<&str, Error> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .ok_or(Error(400))
    }

    /// `html!` calls this to choose how many layouts to render.
    pub fn partial_depth(&self) -> u32 {
        self.held
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

#[derive(Debug)]
pub struct Error(pub u16);

impl From<std::num::ParseIntError> for Error {
    fn from(_: std::num::ParseIntError) -> Self {
        Error(400)
    }
}

pub trait IntoResponse {
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;

pub fn page<H, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
    T: IntoResponse,
{
    Box::new(move |ctx| handler(ctx).map(IntoResponse::into_response))
}

/// What a component is, in this test. rsx's `Template` in a project.
pub trait Render {
    fn render(&self) -> String;
}

/// `html!` hands the outermost component it built to this function.
pub fn render(content: impl Render) -> Result<Response, Error> {
    Ok(Response {
        status: 200,
        body: content.render(),
    })
}

/// The components `rsx::load_dir!` would compile from the `.rsx` files.
pub mod views {
    use super::Render;

    pub mod layout {
        /// `src/pages/layout.rsx`.
        pub struct Layout<C> {
            pub children: C,
        }
        impl<C: super::Render> super::Render for Layout<C> {
            fn render(&self) -> String {
                format!("<html><main>{}</main></html>", self.children.render())
            }
        }
    }

    pub mod index {
        /// `src/pages/index.rsx`.
        pub struct Index;
        impl super::Render for Index {
            fn render(&self) -> String {
                "<h1>Home</h1>".into()
            }
        }
    }

    pub mod users {
        pub mod layout {
            /// `src/pages/users/layout.rsx`.
            pub struct Layout<C> {
                pub children: C,
            }
            impl<C: super::super::Render> super::super::Render for Layout<C> {
                fn render(&self) -> String {
                    format!("<section>{}</section>", self.children.render())
                }
            }
        }

        pub mod _id_ {
            /// `src/pages/users/[id].rsx`.
            pub struct User {
                pub id: u64,
            }
            impl super::super::Render for User {
                fn render(&self) -> String {
                    format!("<h1>User {}</h1>", self.id)
                }
            }
        }
    }

    // Keeps `Render` named at this level for the modules above.
    #[allow(dead_code)]
    fn _uses(_: &dyn Render) {}
}
