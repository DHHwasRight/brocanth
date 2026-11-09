//! [`Flag`], [`Actor`] and [`Rule`]: what a flag is, who asks, and the
//! stored answer. Pure: no store, no clock, no runtime.

/// One flag the application reads, declared in code.
///
/// A generated project lists its flags in `src/plugins/flags/mod.rs`:
///
/// ```
/// use brocanth_flags::Flag;
///
/// pub const NEW_CHECKOUT: Flag = Flag::new("new_checkout", "The checkout flow from the redesign.");
/// pub const ALL: &[Flag] = &[NEW_CHECKOUT];
/// ```
///
/// A page asks with the constant, not with a string, so a misspelt flag is
/// a compile error and not a feature that never turns on. The admin panel
/// lists the same constants, so a flag can be turned on before anybody
/// stored a row for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Flag {
    key: &'static str,
    description: &'static str,
}

impl Flag {
    /// A flag stored under `key`, with a sentence for the admin panel.
    ///
    /// The key is lower case: letters, digits, `_`, `-` and `.`, at most 64
    /// characters. [`valid_key`] checks it; [`crate::Flags::set`] refuses a
    /// key that fails.
    pub const fn new(key: &'static str, description: &'static str) -> Flag {
        Flag { key, description }
    }

    /// The key the row is stored under.
    pub const fn key(&self) -> &'static str {
        self.key
    }

    /// What the flag changes, for the admin panel.
    pub const fn description(&self) -> &'static str {
        self.description
    }
}

/// Whether `key` may name a flag: 1 to 64 characters of `a-z`, `0-9`, `_`,
/// `-` and `.`.
///
/// The rule keeps a key safe in a URL path and in a log line without
/// escaping.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.'))
}

/// Who asks: an id and the groups they are in.
///
/// The application builds it from its own user, in one function of the
/// generated `src/plugins/flags/mod.rs`. The crate has no user type of its
/// own, for the reason `brocanth-admin` has none: who a person is belongs to
/// the application.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Actor {
    id: Option<String>,
    groups: Vec<String>,
}

impl Actor {
    /// Nobody signed in. Only a flag that is on for everyone is on.
    pub fn anonymous() -> Actor {
        Actor::default()
    }

    /// A person with this id: a user id, an account id, an email.
    pub fn new(id: impl Into<String>) -> Actor {
        Actor {
            id: Some(id.into()),
            groups: Vec::new(),
        }
    }

    /// Also in `group`: `staff`, `beta`, an organisation id.
    pub fn in_group(mut self, group: impl Into<String>) -> Actor {
        self.groups.push(group.into());
        self
    }

    /// The id, when somebody is signed in.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The groups.
    pub fn groups(&self) -> &[String] {
        &self.groups
    }
}

/// The stored answer for one flag.
///
/// A flag is on for an actor when any of the four says so:
///
/// 1. `on`: for everyone, the anonymous too.
/// 2. `actors`: the actor's id is in the list.
/// 3. `groups`: one of the actor's groups is in the list.
/// 4. `percent`: the actor's bucket for this flag is below it.
///
/// A flag with no row is off. That is the safe default for code that has
/// shipped before anybody turned it on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    /// On for everyone.
    pub on: bool,
    /// The share of signed-in actors it is on for, 0 to 100.
    pub percent: u8,
    /// Actor ids it is on for.
    pub actors: Vec<String>,
    /// Groups it is on for.
    pub groups: Vec<String>,
}

impl Rule {
    /// On for everyone.
    pub fn everyone() -> Rule {
        Rule {
            on: true,
            ..Rule::default()
        }
    }

    /// On for `percent` of signed-in actors.
    pub fn percent(percent: u8) -> Rule {
        Rule {
            percent: percent.min(100),
            ..Rule::default()
        }
    }

    /// Whether the flag `key` is on for `actor`.
    pub fn allows(&self, key: &str, actor: &Actor) -> bool {
        if self.on {
            return true;
        }
        let Some(id) = actor.id() else {
            // An anonymous request has no stable identity, so a percentage
            // would turn the flag on and off from one page to the next.
            return false;
        };
        if self.actors.iter().any(|a| a == id) {
            return true;
        }
        if actor.groups().iter().any(|g| self.groups.contains(g)) {
            return true;
        }
        self.percent > 0 && bucket(key, id) < u32::from(self.percent.min(100))
    }

    /// Whether the rule turns the flag on for nobody.
    pub fn is_off(&self) -> bool {
        !self.on && self.percent == 0 && self.actors.is_empty() && self.groups.is_empty()
    }

    /// One line for a screen: `On for everyone`, `25% · 2 users · 1 group`,
    /// `Off`.
    pub fn summary(&self) -> String {
        if self.on {
            return "On for everyone".to_string();
        }
        if self.is_off() {
            return "Off".to_string();
        }
        let mut parts = Vec::new();
        if self.percent > 0 {
            parts.push(format!("{}%", self.percent.min(100)));
        }
        let count = |n: usize, one: &str, many: &str| match n {
            1 => format!("1 {one}"),
            n => format!("{n} {many}"),
        };
        if !self.actors.is_empty() {
            parts.push(count(self.actors.len(), "user", "users"));
        }
        if !self.groups.is_empty() {
            parts.push(count(self.groups.len(), "group", "groups"));
        }
        parts.join(" · ")
    }
}

/// The actor's bucket for one flag, 0 to 99.
///
/// FNV-1a over `key`, a NUL and `id`, so one person lands in the same
/// bucket in every process and on every deploy, and two flags at 10% do
/// not pick the same 10% of people. Changing this function moves people
/// in and out of every rollout, so it is pinned by a test.
pub fn bucket(key: &str, id: &str) -> u32 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes().chain([0]).chain(id.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    (hash % 100) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_with_no_rule_is_off_and_a_rule_for_everyone_is_on_for_the_anonymous() {
        assert!(!Rule::default().allows("x", &Actor::new("u1")));
        assert!(Rule::everyone().allows("x", &Actor::anonymous()));
    }

    #[test]
    fn a_listed_actor_or_group_is_on_and_nobody_else_is() {
        let rule = Rule {
            actors: vec!["ada".into()],
            groups: vec!["staff".into()],
            ..Rule::default()
        };
        assert!(rule.allows("x", &Actor::new("ada")));
        assert!(rule.allows("x", &Actor::new("bob").in_group("staff")));
        assert!(!rule.allows("x", &Actor::new("bob").in_group("beta")));
        assert!(!rule.allows("x", &Actor::anonymous()));
    }

    #[test]
    fn a_percentage_is_sticky_per_person_and_close_to_its_share() {
        let rule = Rule::percent(30);
        let on = (0..10_000)
            .filter(|n| rule.allows("checkout", &Actor::new(format!("user-{n}"))))
            .count();
        assert!((2_700..3_300).contains(&on), "{on} of 10000 at 30%");
        let ada = Actor::new("ada");
        let first = rule.allows("checkout", &ada);
        assert!((0..20).all(|_| rule.allows("checkout", &ada) == first));
        // The anonymous have no bucket.
        assert!(!Rule::percent(99).allows("checkout", &Actor::anonymous()));
        assert!(Rule::percent(100).allows("checkout", &Actor::new("anyone")));
    }

    #[test]
    fn each_percentage_lands_close_to_its_share_over_many_ids() {
        // 20 000 ids: a binomial share's standard deviation is under 0.4
        // points, so 1.5 points either way never fails by chance.
        for percent in [1_u8, 10, 25, 50, 75, 90, 99] {
            let rule = Rule::percent(percent);
            let on = (0..20_000)
                .filter(|n| rule.allows("rollout", &Actor::new(format!("{n}"))))
                .count();
            let share = on as f64 / 200.0;
            assert!(
                (share - f64::from(percent)).abs() < 1.5,
                "{percent}% turned on {share}%"
            );
        }
    }

    #[test]
    fn a_person_in_at_ten_percent_stays_in_at_a_higher_percent() {
        // Raising a rollout adds people. It never drops one already in.
        for n in 0..2_000 {
            let actor = Actor::new(format!("user-{n}"));
            let mut was_on = false;
            for percent in 0..=100_u8 {
                let on = Rule::percent(percent).allows("checkout", &actor);
                assert!(!was_on || on, "user-{n} left at {percent}%");
                was_on = on;
            }
        }
    }

    #[test]
    fn two_flags_at_ten_percent_do_not_pick_the_same_people() {
        let rule = Rule::percent(10);
        let ids: Vec<Actor> = (0..10_000)
            .map(|n| Actor::new(format!("user-{n}")))
            .collect();
        let a: Vec<bool> = ids.iter().map(|id| rule.allows("search", id)).collect();
        let b: Vec<bool> = ids.iter().map(|id| rule.allows("checkout", id)).collect();
        let both = a.iter().zip(&b).filter(|(x, y)| **x && **y).count();
        // Independent flags share about 1% of people (100 of 10 000). The
        // same hash for both would share all 10%.
        assert!(both < 250, "{both} people are in both rollouts");
    }

    #[test]
    fn an_anonymous_actor_in_a_group_sees_only_on_for_everyone() {
        let rule = Rule {
            percent: 100,
            actors: vec![String::new()],
            groups: vec!["staff".into()],
            on: false,
        };
        assert!(!rule.allows("x", &Actor::anonymous().in_group("staff")));
        assert!(Rule::everyone().allows("x", &Actor::anonymous().in_group("staff")));
    }

    #[test]
    fn a_percent_over_a_hundred_is_read_as_a_hundred() {
        assert_eq!(Rule::percent(250).percent, 100);
        let rule = Rule {
            percent: 250,
            ..Rule::default()
        };
        assert!(rule.allows("x", &Actor::new("anyone")));
        assert_eq!(rule.summary(), "100%");
    }

    #[test]
    fn a_group_or_user_match_is_exact_not_a_prefix_or_case_fold() {
        let rule = Rule {
            actors: vec!["ada".into()],
            groups: vec!["staff".into()],
            ..Rule::default()
        };
        assert!(!rule.allows("x", &Actor::new("Ada")));
        assert!(!rule.allows("x", &Actor::new("ada2")));
        assert!(!rule.allows("x", &Actor::new("bob").in_group("Staff")));
        assert!(!rule.allows("x", &Actor::new("bob").in_group("staff-old")));
    }

    #[test]
    fn the_bucket_function_is_pinned() {
        // A change here moves people in and out of every live rollout. The
        // values are FNV-1a 64 of `key \0 id`, mod 100, checked by hand.
        assert_eq!(bucket("checkout", "ada"), bucket("checkout", "ada"));
        assert_eq!(
            [bucket("a", "1"), bucket("checkout", "ada"), bucket("x", "")],
            [PINNED_A1, PINNED_CHECKOUT_ADA, PINNED_X]
        );
    }

    const PINNED_A1: u32 = 23;
    const PINNED_CHECKOUT_ADA: u32 = 1;
    const PINNED_X: u32 = 9;

    #[test]
    fn a_key_is_short_lower_case_and_safe_in_a_path() {
        assert!(valid_key("new_checkout"));
        assert!(valid_key("billing.v2-beta"));
        assert!(!valid_key(""));
        assert!(!valid_key("New"));
        assert!(!valid_key("a/b"));
        assert!(!valid_key(&"a".repeat(65)));
    }

    #[test]
    fn a_summary_names_each_part() {
        assert_eq!(Rule::default().summary(), "Off");
        assert_eq!(Rule::everyone().summary(), "On for everyone");
        let rule = Rule {
            percent: 25,
            actors: vec!["a".into(), "b".into()],
            groups: vec!["staff".into()],
            on: false,
        };
        assert_eq!(rule.summary(), "25% · 2 users · 1 group");
    }
}
