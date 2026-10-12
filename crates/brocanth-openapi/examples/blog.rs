//! A small application, end to end: a route table, a derived document, the
//! plugin that serves it, and the drift check a CI job runs.
//!
//! ```text
//! cargo run -p brocanth-openapi --example blog            # print the JSON
//! cargo run -p brocanth-openapi --example blog -- --yaml  # print the YAML
//! cargo run -p brocanth-openapi --example blog -- --check openapi.json
//! cargo run -p brocanth-openapi --example blog -- --write openapi.json
//! ```
//!
//! The last two are the pair that matters. `--write` is what a developer runs
//! after changing a route; `--check` is what CI runs, and it fails the build
//! when the two disagree.

use std::process::ExitCode;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::routing::{Router, RouterBuilder, Verb};
use brocanth_openapi::{
    check_file, write, Document, Format, OpenApiConfig, OpenApiPlugin, Schema, SchemaSource,
    SecurityScheme, Server, SpecBuilder, Tag,
};

// ---------------------------------------------------------------------------
// The application's types, describing themselves
// ---------------------------------------------------------------------------

/// A published post.
struct Post;

impl SchemaSource for Post {
    fn schema_name() -> Option<&'static str> {
        Some("Post")
    }

    fn schema() -> Schema {
        Schema::object()
            .required("id", Schema::integer().minimum(1.0))
            .required("title", Schema::string().max_length(120))
            .required("body", Schema::string().nullable())
            .required(
                "status",
                Schema::string().string_enum(["draft", "published"]),
            )
            .property(
                "published_at",
                Schema::string().format("date-time").nullable(),
            )
    }
}

/// What the create and update forms send.
struct PostForm;

impl SchemaSource for PostForm {
    fn schema_name() -> Option<&'static str> {
        Some("PostForm")
    }

    fn schema() -> Schema {
        Schema::object()
            .required("title", Schema::string().min_length(1).max_length(120))
            .property("body", Schema::string())
    }
}

/// A comment on a post.
struct Comment;

impl SchemaSource for Comment {
    fn schema_name() -> Option<&'static str> {
        Some("Comment")
    }

    fn schema() -> Schema {
        Schema::object()
            .required("id", Schema::integer())
            .required("post", Schema::reference("Post"))
            .required("author", Schema::string())
            .required("body", Schema::string())
    }

    fn dependencies() -> Vec<brocanth_openapi::NamedSchema> {
        vec![brocanth_openapi::NamedSchema::new("Post", Post::schema())]
    }
}

/// The shape every error response takes.
struct ErrorBody;

impl SchemaSource for ErrorBody {
    fn schema_name() -> Option<&'static str> {
        Some("Error")
    }

    fn schema() -> Schema {
        Schema::object()
            .required("message", Schema::string())
            .property(
                "fields",
                Schema::object()
                    .additional_properties(brocanth_openapi::Additional::Schema(Box::new(
                        Schema::array(Schema::string()),
                    )))
                    .description("One entry per field that failed validation."),
            )
    }
}

// ---------------------------------------------------------------------------
// The route table
// ---------------------------------------------------------------------------

/// What the server dispatches on. In a real application this is generated
/// from `src/pages`; here it is written out so the example stands alone.
fn router() -> Router<&'static str> {
    let mut routes: RouterBuilder<&'static str> = Router::builder();
    routes.route("/", Verb::Get, "home").unwrap();
    routes.route("/posts", Verb::Get, "posts#index").unwrap();
    routes.route("/posts", Verb::Post, "posts#create").unwrap();
    routes.route("/posts/{id}", Verb::Get, "posts#show").unwrap();
    routes
        .route("/posts/{id}", Verb::Patch, "posts#update")
        .unwrap();
    routes
        .route("/posts/{id}", Verb::Delete, "posts#destroy")
        .unwrap();
    routes
        .route("/posts/{id}/comments", Verb::Get, "comments#index")
        .unwrap();
    routes
        .route("/posts/{id}/comments", Verb::Post, "comments#create")
        .unwrap();
    routes.route("/docs/{*rest}", Verb::Get, "docs#show").unwrap();
    routes.route("/openapi.json", Verb::Get, "openapi").unwrap();
    routes.route("/openapi.yaml", Verb::Get, "openapi").unwrap();
    routes.build().unwrap()
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

fn document() -> Document {
    SpecBuilder::new("Blog", "1.0.0")
        .summary("Posts and the comments on them.")
        .description(
            "Every path, verb and path parameter here comes from the route table the \
             server dispatches on, so the two cannot disagree.",
        )
        .server(Server::new("https://blog.example.com").described("production"))
        .server(Server::new("http://localhost:3000").described("development"))
        .tag(Tag::new("posts").described("Writing and reading posts."))
        .tag(Tag::new("comments").described("What readers said."))
        .security_scheme("session", SecurityScheme::session_cookie("_blog_session"))
        .component_of::<Post>()
        .component_of::<PostForm>()
        .component_of::<Comment>()
        .component_of::<ErrorBody>()
        .routes_from_router(&router())
        // The document does not describe the routes that serve the document.
        .hide("/openapi.json", Verb::Get)
        .hide("/openapi.yaml", Verb::Get)
        .describe("/", Verb::Get, |op| {
            op.summary("The front page")
                .tag("pages")
                .html_response(200, "The rendered front page")
        })
        .describe("/posts", Verb::Get, |op| {
            op.summary("List posts")
                .description("Newest first. Twenty to a page.")
                .query_param("page", Schema::integer().minimum(1.0).default_value(1_i64))
                .query_param(
                    "status",
                    Schema::string().string_enum(["draft", "published"]),
                )
                .json_response(
                    200,
                    "A page of posts",
                    Schema::array(Schema::reference("Post")),
                )
        })
        .describe("/posts", Verb::Post, |op| {
            op.summary("Create a post")
                .tag("posts")
                .security(brocanth_openapi::SecurityRequirement::new("session"))
                .form_body(Schema::reference("PostForm"))
                .json_response(201, "The post that was created", Schema::reference("Post"))
                .json_response(
                    422,
                    "The form did not validate; the page is re-rendered with the errors",
                    Schema::reference("Error"),
                )
        })
        .describe("/posts/{id}", Verb::Get, |op| {
            op.summary("Show one post")
                .path_param_described("id", Schema::integer(), "The post's primary key")
                .json_response(200, "The post", Schema::reference("Post"))
                .json_response(404, "No post has that id", Schema::reference("Error"))
        })
        .describe("/posts/{id}", Verb::Patch, |op| {
            op.summary("Update a post")
                .path_param("id", Schema::integer())
                .security(brocanth_openapi::SecurityRequirement::new("session"))
                .form_body(Schema::reference("PostForm"))
                .json_response(200, "The post after the change", Schema::reference("Post"))
                .json_response(422, "The form did not validate", Schema::reference("Error"))
        })
        .describe("/posts/{id}", Verb::Delete, |op| {
            op.summary("Delete a post")
                .path_param("id", Schema::integer())
                .security(brocanth_openapi::SecurityRequirement::new("session"))
                .status(204, "The post is gone")
                .json_response(404, "No post has that id", Schema::reference("Error"))
        })
        .describe("/posts/{id}/comments", Verb::Get, |op| {
            op.summary("List a post's comments")
                .tag("comments")
                .path_param("id", Schema::integer())
                .json_response(
                    200,
                    "Every comment on the post",
                    Schema::array(Schema::reference("Comment")),
                )
        })
        .describe("/posts/{id}/comments", Verb::Post, |op| {
            op.summary("Comment on a post")
                .tag("comments")
                .path_param("id", Schema::integer())
                .form_body(
                    Schema::object()
                        .required("author", Schema::string())
                        .required("body", Schema::string().min_length(1)),
                )
                .json_response(201, "The comment", Schema::reference("Comment"))
                .json_response(422, "The form did not validate", Schema::reference("Error"))
        })
        .describe("/docs/{*rest}", Verb::Get, |op| {
            op.summary("A documentation page")
                .tag("pages")
                .html_response(200, "The rendered page")
                .status(404, "No page lives at that path")
        })
        .build()
        .expect("the document derives from the route table")
}

// ---------------------------------------------------------------------------
// Wiring, and the two commands
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    let document = document();

    // The plugin is named in `main`, like every other plugin. An application
    // that deletes this line links no part of `brocanth-openapi`.
    let app = AppBuilder::new()
        .plugin(OpenApiPlugin::new(OpenApiConfig::new(document.clone())))
        .build()
        .expect("the document validates during `configure`");
    let plugin = app.plugin::<OpenApiPlugin>().expect("just registered");

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--check") => {
            let path = args.get(1).map_or("openapi.json", String::as_str);
            match check_file(plugin.document(), path) {
                Ok(()) => {
                    println!("{path} is up to date.");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--write") => {
            let path = args.get(1).map_or("openapi.json", String::as_str);
            match write(plugin.document(), path) {
                Ok(()) => {
                    println!("wrote {path}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{path}: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--yaml") => {
            print!("{}", plugin.body(Format::Yaml));
            ExitCode::SUCCESS
        }
        Some("--serve") => {
            // What the server layer will call once it exists.
            let response = plugin.serve("/openapi.json").expect("mounted there");
            println!("{} {:?}", response.status(), response.headers());
            ExitCode::SUCCESS
        }
        _ => {
            print!("{}", plugin.body(Format::Json));
            ExitCode::SUCCESS
        }
    }
}
