//! Deterministic hook order, declared rather than discovered.
//!
//! Two plugins hooking one point must run in an order that is a decision
//! somebody wrote down, not a consequence of which `use` came first. A hook
//! therefore carries an [`Order`]: a list of hooks it must run after, and a
//! list it must run before. [`sort`] turns those declarations into one total
//! order, or fails and names what is wrong.
//!
//! Three properties, all tested below.
//!
//! 1. **Declarations win.** If `a` says `after("b")`, `b` runs first, whatever
//!    order they were registered in.
//! 2. **Registration order is only the tie-break.** Two hooks with nothing to
//!    say about each other keep the order they were registered in, so the
//!    result is stable across runs and across rebuilds. It is a tie-break and
//!    never a mechanism: a plugin that cares says so.
//! 3. **A cycle is an error that names both ends**, and prints the path
//!    between them, because `a -> b -> c -> a` reported as "cycle detected" is
//!    a bug report you cannot act on.
//!
//! A reference is either a whole instance (`"auth"`, meaning every hook that
//! instance registered) or one hook (`"auth.session"`). The instance form is
//! what a plugin almost always wants: "after everything the session plugin
//! does" survives the session plugin growing a second hook.
//!
//! ```
//! use brocanth::plugin_order::{sort, HookId, Order, Ordered};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let items = vec![
//!     Ordered::new(HookId::new("audit", "trail"), Order::new().after("auth"), "audit"),
//!     Ordered::new(HookId::new("auth", "session"), Order::new(), "auth"),
//! ];
//! assert_eq!(sort(items)?, vec!["auth", "audit"]);
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;

use thiserror::Error;

use crate::suggest::nearest_name;

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// What one hook is called: the instance that registered it, and a name that
/// instance chose.
///
/// The instance half is filled in by the core, never by the plugin, so a hook
/// cannot take a name that belongs to somebody else's instance.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HookId {
    instance: String,
    name: String,
}

impl HookId {
    pub fn new(instance: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            instance: instance.into(),
            name: name.into(),
        }
    }

    /// The instance that registered the hook.
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// The name the instance gave it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `instance.name`, which is how an [`Order`] refers to one hook.
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.instance, self.name)
    }
}

impl std::fmt::Display for HookId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.instance, self.name)
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Dep {
    target: String,
    required: bool,
}

/// Where a hook sits relative to other hooks.
///
/// Empty by default, which means "anywhere, keep my registration order".
/// `after` and `before` take either an instance name or an `instance.hook`
/// pair.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Order {
    after: Vec<Dep>,
    before: Vec<Dep>,
}

impl Order {
    /// No constraints.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run after `who`. The build fails if nothing answers to that name,
    /// because "after the thing that is not here" is almost always a typo.
    #[must_use]
    pub fn after(mut self, who: impl Into<String>) -> Self {
        self.after.push(Dep {
            target: who.into(),
            required: true,
        });
        self
    }

    /// Run after `who` when `who` is installed, and say nothing otherwise.
    ///
    /// For the genuinely optional relationship: a cache plugin that must come
    /// after the session plugin *if the application has one*.
    #[must_use]
    pub fn after_if_present(mut self, who: impl Into<String>) -> Self {
        self.after.push(Dep {
            target: who.into(),
            required: false,
        });
        self
    }

    /// Run before `who`. The build fails if nothing answers to that name.
    #[must_use]
    pub fn before(mut self, who: impl Into<String>) -> Self {
        self.before.push(Dep {
            target: who.into(),
            required: true,
        });
        self
    }

    /// Run before `who` when `who` is installed.
    #[must_use]
    pub fn before_if_present(mut self, who: impl Into<String>) -> Self {
        self.before.push(Dep {
            target: who.into(),
            required: false,
        });
        self
    }

    /// The names this hook asked to follow.
    pub fn after_names(&self) -> impl Iterator<Item = &str> {
        self.after.iter().map(|d| d.target.as_str())
    }

    /// The names this hook asked to precede.
    pub fn before_names(&self) -> impl Iterator<Item = &str> {
        self.before.iter().map(|d| d.target.as_str())
    }

    /// Whether the hook said nothing at all.
    pub fn is_unconstrained(&self) -> bool {
        self.after.is_empty() && self.before.is_empty()
    }
}

/// One thing to be sorted: an identity, its declarations, and the value.
pub struct Ordered<T> {
    pub id: HookId,
    pub order: Order,
    pub value: T,
}

impl<T> Ordered<T> {
    pub fn new(id: HookId, order: Order, value: T) -> Self {
        Self { id, order, value }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Ordered<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ordered")
            .field("id", &self.id.to_string())
            .field("order", &self.order)
            .field("value", &self.value)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum OrderError {
    #[error(
        "`{first}` must run after `{second}`, and `{second}` must run after `{first}` \
         (through {}); one of those two declarations has to go",
        .path.join(" -> ")
    )]
    Cycle {
        /// One end of the cycle.
        first: String,
        /// The other end.
        second: String,
        /// The whole loop, starting and ending at `first`.
        path: Vec<String>,
    },

    #[error(
        "`{hook}` declares `{relation} {depends_on}`, which nothing answers to{}; registered: {}",
        crate::plugin_order::order_hint(.suggestion),
        if .known.is_empty() { "nothing".to_string() } else { .known.join(", ") }
    )]
    Unknown {
        hook: String,
        relation: &'static str,
        depends_on: String,
        suggestion: Option<String>,
        known: Vec<String>,
    },
}

#[doc(hidden)]
pub fn order_hint(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(s) => format!("; did you mean `{s}`?"),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// The sort
// ---------------------------------------------------------------------------

/// Puts hooks in the one order their declarations allow.
///
/// Registration order is the tie-break, so the result is the same on every
/// run and every rebuild.
///
/// # Errors
///
/// [`OrderError::Unknown`] when a required `after`/`before` names nothing, and
/// [`OrderError::Cycle`] when the declarations contradict each other.
pub fn sort<T>(items: Vec<Ordered<T>>) -> Result<Vec<T>, OrderError> {
    let n = items.len();
    if n < 2 {
        return Ok(items.into_iter().map(|o| o.value).collect());
    }

    // Every name a declaration may legally use, and what it resolves to.
    let mut targets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        targets.entry(item.id.qualified()).or_default().push(i);
        targets
            .entry(item.id.instance().to_string())
            .or_default()
            .push(i);
    }

    // `edges[a]` holds every hook that must run after `a`.
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indegree: Vec<usize> = vec![0; n];
    fn add(from: usize, to: usize, edges: &mut [Vec<usize>], indeg: &mut [usize]) {
        if from != to && !edges[from].contains(&to) {
            edges[from].push(to);
            indeg[to] += 1;
        }
    }

    for (i, item) in items.iter().enumerate() {
        for (relation, deps) in [("after", &item.order.after), ("before", &item.order.before)] {
            for dep in deps {
                match targets.get(&dep.target) {
                    Some(hits) => {
                        for &j in hits {
                            if relation == "after" {
                                add(j, i, &mut edges, &mut indegree);
                            } else {
                                add(i, j, &mut edges, &mut indegree);
                            }
                        }
                    }
                    None if dep.required => {
                        let mut known: Vec<String> = targets.keys().cloned().collect();
                        known.sort_unstable();
                        return Err(OrderError::Unknown {
                            hook: item.id.to_string(),
                            relation,
                            depends_on: dep.target.clone(),
                            suggestion: nearest_name(&dep.target, &known),
                            known,
                        });
                    }
                    None => {}
                }
            }
        }
    }

    // Kahn, always taking the lowest registration index that is ready, so the
    // order is a function of the declarations and nothing else.
    let mut ready: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut out: Vec<usize> = Vec::with_capacity(n);
    while !ready.is_empty() {
        ready.sort_unstable();
        let next = ready.remove(0);
        out.push(next);
        for &to in &edges[next] {
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.push(to);
            }
        }
    }

    if out.len() != n {
        let stuck: Vec<usize> = (0..n).filter(|&i| indegree[i] > 0).collect();
        return Err(cycle_error(&items, &edges, &stuck));
    }

    // Reorder without cloning: take each value out by index.
    let mut slots: Vec<Option<T>> = items.into_iter().map(|o| Some(o.value)).collect();
    Ok(out
        .into_iter()
        .map(|i| slots[i].take().expect("each index appears once"))
        .collect())
}

/// Walks the remaining graph until it revisits a node, which is the cycle.
fn cycle_error<T>(items: &[Ordered<T>], edges: &[Vec<usize>], stuck: &[usize]) -> OrderError {
    let start = stuck[0];
    let mut path = vec![start];
    let mut seen = vec![false; items.len()];
    seen[start] = true;
    let mut cur = start;
    while let Some(&next) = edges[cur].iter().find(|&&j| stuck.contains(&j)) {
        if seen[next] {
            // Trim everything before the first visit, so the path is the loop.
            let at = path.iter().position(|&p| p == next).unwrap_or(0);
            path = path[at..].to_vec();
            path.push(next);
            break;
        }
        seen[next] = true;
        path.push(next);
        cur = next;
    }

    let names: Vec<String> = path.iter().map(|&i| items[i].id.to_string()).collect();
    let first = names.first().cloned().unwrap_or_default();
    let second = names.get(1).cloned().unwrap_or_else(|| first.clone());
    OrderError::Cycle {
        first,
        second,
        path: names,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn item(instance: &str, name: &str, order: Order) -> Ordered<String> {
        Ordered::new(
            HookId::new(instance, name),
            order,
            format!("{instance}.{name}"),
        )
    }

    #[test]
    fn nothing_declared_keeps_registration_order() {
        let got = sort(vec![
            item("c", "h", Order::new()),
            item("a", "h", Order::new()),
            item("b", "h", Order::new()),
        ])
        .unwrap();
        assert_eq!(got, vec!["c.h", "a.h", "b.h"]);
    }

    #[test]
    fn after_beats_registration_order() {
        let got = sort(vec![
            item("audit", "trail", Order::new().after("auth")),
            item("auth", "session", Order::new()),
        ])
        .unwrap();
        assert_eq!(got, vec!["auth.session", "audit.trail"]);
    }

    #[test]
    fn before_beats_registration_order_too() {
        let got = sort(vec![
            item("auth", "session", Order::new()),
            item("trace", "id", Order::new().before("auth")),
        ])
        .unwrap();
        assert_eq!(got, vec!["trace.id", "auth.session"]);
    }

    #[test]
    fn an_instance_name_covers_every_hook_that_instance_registered() {
        let got = sort(vec![
            item("late", "h", Order::new().after("auth")),
            item("auth", "one", Order::new()),
            item("auth", "two", Order::new()),
        ])
        .unwrap();
        assert_eq!(got, vec!["auth.one", "auth.two", "late.h"]);
    }

    #[test]
    fn a_qualified_name_picks_out_one_hook() {
        let got = sort(vec![
            item("late", "h", Order::new().after("auth.two")),
            item("auth", "one", Order::new()),
            item("auth", "two", Order::new()),
        ])
        .unwrap();
        assert_eq!(got, vec!["auth.one", "auth.two", "late.h"]);
    }

    #[test]
    fn the_order_is_the_same_however_the_input_is_shuffled() {
        let wanted = vec!["a.h", "b.h", "c.h"];
        for shuffle in [[0, 1, 2], [2, 1, 0], [1, 2, 0], [2, 0, 1]] {
            let mut given = [
                item("a", "h", Order::new()),
                item("b", "h", Order::new().after("a")),
                item("c", "h", Order::new().after("b")),
            ];
            let mut taken: Vec<Ordered<String>> = Vec::new();
            for i in shuffle {
                taken.push(std::mem::replace(
                    &mut given[i],
                    item("filler", "x", Order::new()),
                ));
            }
            // drop the fillers, keep the three real ones in the shuffled order
            let got = sort(taken).unwrap();
            assert_eq!(got, wanted, "shuffle {shuffle:?}");
        }
    }

    #[test]
    fn a_cycle_names_both_ends_and_prints_the_loop() {
        let err = sort(vec![
            item("a", "h", Order::new().after("c")),
            item("b", "h", Order::new().after("a")),
            item("c", "h", Order::new().after("b")),
        ])
        .unwrap_err();

        let OrderError::Cycle {
            ref first,
            ref second,
            ref path,
        } = err
        else {
            panic!("expected a cycle, got {err}");
        };
        assert_ne!(first, second);
        assert_eq!(path.first(), path.last());
        assert_eq!(path.len(), 4, "a -> b -> c -> a");
        let text = err.to_string();
        assert!(text.contains(first), "{text}");
        assert!(text.contains(second), "{text}");
    }

    #[test]
    fn the_shortest_cycle_is_still_named_in_full() {
        let err = sort(vec![
            item("a", "h", Order::new().after("b")),
            item("b", "h", Order::new().after("a")),
        ])
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("`a.h`"), "{text}");
        assert!(text.contains("`b.h`"), "{text}");
    }

    #[test]
    fn a_required_dependency_that_is_not_installed_is_an_error_with_a_suggestion() {
        let err = sort(vec![
            item("audit", "trail", Order::new().after("autth")),
            item("auth", "session", Order::new()),
        ])
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("did you mean `auth`"), "{text}");
    }

    #[test]
    fn an_optional_dependency_that_is_not_installed_is_silence() {
        let got = sort(vec![
            item("cache", "warm", Order::new().after_if_present("session")),
            item("auth", "session_free", Order::new()),
        ])
        .unwrap();
        assert_eq!(got, vec!["cache.warm", "auth.session_free"]);
    }

    #[test]
    fn one_item_needs_no_graph() {
        let got = sort(vec![item("a", "h", Order::new().after("nobody"))]).unwrap();
        assert_eq!(got, vec!["a.h"]);
    }

    #[test]
    fn a_hook_cannot_order_itself_into_a_cycle() {
        let got = sort(vec![
            item("a", "h", Order::new().after("a")),
            item("b", "h", Order::new()),
        ])
        .unwrap();
        assert_eq!(got, vec!["a.h", "b.h"]);
    }
}
