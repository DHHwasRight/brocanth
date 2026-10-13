//! The evidence for the claim that a hook nobody registers costs nothing.
//!
//! The claim has three parts, and prose is not evidence for any of them, so
//! each is checked here.
//!
//! 1. **No branch on an empty list.** The first test asks `rustc` for the LLVM
//!    IR of the core and reads the bodies it generated for the probes in
//!    `plugin_hooks::probe`. With the family record saying "nobody installed a
//!    request hook", `before_dispatch` must be a single `ret` — not a load of
//!    the `Vec`'s length and a compare against zero. The same probe compiled
//!    with the record saying the opposite must contain the loop, so the test
//!    cannot pass by measuring nothing.
//! 2. **No allocation.** The second test counts every allocation on the
//!    calling thread and asserts that running a request through an application
//!    with no hooks allocates exactly as many times as calling the handler
//!    directly, which is zero.
//! 3. **No dynamic dispatch.** Falls out of (1): there is no `call` in the
//!    generated body at all, let alone an indirect one.
//!
//! The IR test shells out to `cargo rustc` with its own target directory, so
//! it never contends with the `cargo test` that is running it. It takes a few
//! seconds the first time and is incremental afterwards.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// 1. The generated code
// ---------------------------------------------------------------------------

/// Probes that must compile to nothing, and the family record each was
/// compiled with.
const MUST_BE_FREE: &[&str] = &[
    "empty_before_dispatch",
    "empty_async_before_dispatch",
    "empty_on_response",
    "empty_after_dispatch",
    "empty_model_emit",
];

/// The same seams with a hook installed. If these were also free the test
/// above would be measuring nothing.
const MUST_NOT_BE_FREE: &[&str] = &[
    "populated_before_dispatch",
    "populated_async_before_dispatch",
    "populated_on_response",
    "populated_model_emit",
];

#[test]
fn a_hook_nobody_registered_is_not_in_the_generated_code() {
    let ir = llvm_ir();

    for name in MUST_BE_FREE {
        let body = function_body(&ir, name)
            .unwrap_or_else(|| panic!("no `{name}` in the generated IR; did the probe move?"));

        let instructions: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with(';') && !l.ends_with(':'))
            .collect();

        assert!(
            instructions.len() <= 2,
            "`{name}` compiled to {} instructions; a hook nobody registered has to be a \
             `ret` and nothing else:\n{}",
            instructions.len(),
            body
        );
        for forbidden in ["call", "invoke", "alloca", "br ", "load ", "icmp"] {
            assert!(
                !instructions.iter().any(|l| l.contains(forbidden)),
                "`{name}` contains `{forbidden}`, so the empty case is still being looked \
                 at:\n{body}"
            );
        }
        assert!(
            instructions.iter().any(|l| l.starts_with("ret")),
            "`{name}` does not return:\n{body}"
        );
    }

    for name in MUST_NOT_BE_FREE {
        let body = function_body(&ir, name)
            .unwrap_or_else(|| panic!("no `{name}` in the generated IR; did the probe move?"));
        assert!(
            body.contains("br ") || body.contains("call"),
            "`{name}` has no loop and no call, so the comparison above proves nothing:\n{body}"
        );
    }
}

/// The IR `rustc` generates for the core, compiled in release with one
/// codegen unit so the whole crate lands in one file.
fn llvm_ir() -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let target = std::env::temp_dir().join("brocanth-llvm-ir-probe");

    let out = Command::new(&cargo)
        .env("CARGO_TARGET_DIR", &target)
        .args(["rustc", "--release", "--lib", "--manifest-path"])
        .arg(manifest)
        .args(["--", "-C", "codegen-units=1", "--emit=llvm-ir"])
        .output()
        .unwrap_or_else(|e| panic!("cannot run `{cargo} rustc`: {e}"));
    assert!(
        out.status.success(),
        "`cargo rustc --emit=llvm-ir` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let deps = target.join("release").join("deps");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&deps)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", deps.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "ll")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("brocanth"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .ll file in {}", deps.display());

    files
        .iter()
        .map(|p| std::fs::read_to_string(p).expect("unreadable .ll"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the one `define` whose symbol contains `name`.
///
/// Follows an `alias`, because LLVM folds identical functions: when a probe
/// compiles to the same instructions as another probe, the second symbol
/// becomes `@a = unnamed_addr alias .., ptr @b` and there is no `define` for
/// it. Reading that as "the probe moved" would throw away the result, and the
/// result is the strongest possible pass -- the empty case was not merely
/// small, it was byte-identical to a case already known to be free. So this
/// strengthens the test rather than relaxing it: a probe can no longer escape
/// inspection by being optimised into somebody else's symbol.
///
/// Measured while prototyping async handlers: an empty *async* hook family
/// folded into `empty_before_dispatch`, whose body is `ret i1 true`. See
/// `docs/async-handlers.md` §5.
fn function_body(ir: &str, name: &str) -> Option<String> {
    for line in ir.lines() {
        let Some((lhs, rhs)) = line.split_once(" = ") else {
            continue;
        };
        if !lhs.starts_with('@') || !lhs.contains(name) || !rhs.contains(" alias ") {
            continue;
        }
        let target = rhs.rsplit_once("ptr @")?.1.trim();
        return function_body(ir, target);
    }
    body_of_define(ir, name)
}

fn body_of_define(ir: &str, name: &str) -> Option<String> {
    let mut lines = ir.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("define") || !line.contains(name) {
            continue;
        }
        let mut body = String::new();
        for line in lines.by_ref() {
            if line == "}" {
                return Some(body);
            }
            body.push_str(line);
            body.push('\n');
        }
        return Some(body);
    }
    None
}

// ---------------------------------------------------------------------------
// 2. The allocations
// ---------------------------------------------------------------------------

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = ALLOCATIONS.with(Cell::get);
    let value = f();
    let after = ALLOCATIONS.with(Cell::get);
    (value, after - before)
}

#[test]
fn a_request_through_an_application_with_no_hooks_allocates_nothing() {
    use brocanth::plugin_app::AppBuilder;
    use brocanth::plugin_hooks::{HandlerError, Request, RequestState, Response};

    let app = AppBuilder::new().build().unwrap();
    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();

    let handler = |_: &mut Request, _: &mut RequestState| -> Result<Response, HandlerError> {
        Ok(Response::new(Vec::new()))
    };

    // Warm anything the first call might set up, so the measurement is of the
    // steady state rather than of a one-off.
    let _ = app.run(&mut request, &mut state, handler);

    let (_, direct) = allocations_during(|| handler(&mut request, &mut state).unwrap());
    let (_, through) = allocations_during(|| app.run(&mut request, &mut state, handler));

    assert_eq!(
        direct, 0,
        "the baseline itself allocates, so the comparison would prove nothing"
    );
    assert_eq!(
        through, direct,
        "running a request through an application with no hooks allocated {through} time(s); \
         the pipeline has to be free when nothing registered it"
    );
}

#[test]
fn an_empty_request_state_and_an_empty_pipeline_allocate_nothing_to_exist() {
    use brocanth::plugin_hooks::{Pipeline, RequestState};

    let (_, state) = allocations_during(RequestState::new);
    assert_eq!(state, 0, "an empty `RequestState` allocated");

    let (_, pipeline) = allocations_during(Pipeline::default);
    assert_eq!(pipeline, 0, "an empty `Pipeline` allocated");
}
