# brocanth-content

Content collections for brocanth: a directory of Markdown files with typed
front matter, checked by the build script and rendered once at start, and
an Atom feed of a collection. No database: the binary embeds the files.

## Getting started

```text
brocanth create my-blog --with rsx,markdown,content,feed
```

The project gets `content/posts/hello.md`, its front matter type in
`src/plugins/content/posts.rs`, a step in `build.rs`, and the pages
`/posts` and `/posts/{slug}`.

## The build step

```rust,ignore
fn main() {
    #[allow(dead_code)]
    #[path = "src/plugins/content/posts.rs"]
    mod posts;
    brocanth_content::build::collection::<posts::Post>("content/posts")
        .unwrap_or_else(|e| panic!("{e}"));
}
```

A bad file stops the build with its path, its line and a `help:` line.
The step writes `$OUT_DIR/content_posts.rs`, which the binary includes.

## At run time

```rust,ignore
let posts = Collection::<Post>::parse("/posts", sources::SOURCES)?;
let view = posts.view(false); // drafts hidden
for post in view.list(None) {
    println!("{} {}", post.path, post.data.title);
}
let xml = brocanth_content::feed::Atom::new("https://example.com", "/feed.xml", "Blog", "Ann")?
    .entries(posts.published(), 20)
    .render();
```

## Features

- `build`: what `build.rs` links. No Markdown or HTML parser.
- `runtime` (default): the collection and the feed.
- `scaffold`: the `content` and `feed` modules.

See `docs/modules/content.md` and `docs/modules/feed.md`.
