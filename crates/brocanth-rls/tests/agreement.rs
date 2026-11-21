//! The agreement harness (docs/policy-rls-bridge.md decision 10) over
//! fixed rule sets: every row of the translation table (§1), guard chains,
//! `exists` and `!exists`, `update_check`, `else null`, `uuid` and
//! `timestamptz` columns and attributes, unguarded write masks, and a
//! forgotten `update_check` on a status change. Each
//! policy runs through
//! `agreement::check`; two run through `policy!`'s `db { … }` clause.
//!
//! Skipped without `BROCANTH_TEST_DATABASE_URL` (a superuser URL).
#![allow(missing_docs)]

use brocanth_rls::agreement::{check, test_database_url};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Debug, Default, brocanth_policy::Subject)]
pub struct Who {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub tenant_id: i64,
    #[attr]
    pub admin: bool,
    #[attr]
    pub banned: bool,
    #[attr]
    pub role: String,
    #[attr]
    pub team: Option<Uuid>,
    #[attr]
    pub since: Option<DateTime<Utc>>,
}

impl Who {
    /// A signed-in user. The role is not empty: an empty text attribute is
    /// absent, and a guard over an absent attribute denies the whole rule.
    fn user(id: i64) -> Who {
        Who {
            user_id: Some(id),
            tenant_id: 1,
            role: "member".to_string(),
            ..Who::default()
        }
    }

    fn role(id: i64, role: &str) -> Who {
        Who {
            role: role.to_string(),
            ..Who::user(id)
        }
    }

    fn admin() -> Who {
        Who {
            admin: true,
            ..Who::user(0)
        }
    }

    fn banned(id: i64) -> Who {
        Who {
            banned: true,
            ..Who::user(id)
        }
    }

    fn team(id: i64, team: u128, since: i64) -> Who {
        Who {
            team: Some(Uuid::from_u128(team)),
            since: DateTime::from_timestamp_micros(since),
            ..Who::user(id)
        }
    }
}

const EPOCH: DateTime<Utc> = DateTime::<Utc>::UNIX_EPOCH;
const NIL: Uuid = Uuid::nil();

brocanth_policy::policy! {
    pub entity Doc in "docs" {
        #[key] id: i64,
        author_id: Option<i64>,
        tenant_id: i64,
        status: String,
        score: i32,
        flag: Option<bool>,
        team_id: Option<Uuid>,
        published_at: Option<DateTime<Utc>>,
    }

    pub entity Member in "members" {
        #[key] doc_id: i64,
        user_id: Option<i64>,
        role: Option<String>,
    }

    pub entity Block in "blocks" {
        #[key] blocked_id: i64,
        blocker_id: Option<i64>,
    }

    /// Literals only: `always`, `never`, each operator, `is`, `is null`,
    /// `in`, `not in`, `&&`, `||`, `!`.
    #[db]
    pub policy Literals(_: &Who) for Doc {
        list: status == "published" || score > 1 && !(score >= 4) || flag is true,
        view: author_id is null || status != "draft" && score < 1,
        create: status in ["draft", "review"] && score <= 2 || flag is not null && author_id is not 3,
        update: (status == "published" || flag is true) && status not in ["archived"] && always,
        update_check: status == "published" || flag is true,
        delete: status == "published" && score != 2 || never,
    }

    /// Attributes: `else deny`, `else null`, a non-`Option` attribute, and
    /// guard chains over `who.f`, `!who.f`, `who.f == lit`, `&&`, `||`.
    #[db]
    pub policy Attrs(who: &Who) for Doc {
        list {
            who.banned => never,
            who.admin && !who.banned => always,
            who.role == "editor" || who.role == "lead" => tenant_id == who.tenant_id,
            !who.banned => tenant_id == who.tenant_id && (status == "published" || author_id == who.user_id else deny),
        }
        view: author_id == who.user_id else null,
        create: tenant_id == who.tenant_id && author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        update_check {
            who.admin => always,
            author_id == who.user_id else deny && status in ["draft", "published"],
        }
        delete {
            who.admin => always,
            who.role == "lead" => tenant_id == who.tenant_id && status == "draft",
        }

        db {
            subjects: [
                Who::default(),
                Who::user(1),
                Who::user(2),
                Who::role(1, "editor"),
                Who::role(2, "lead"),
                Who::admin(),
                Who::banned(1),
            ],
            cases: 32,
        }
    }

    /// Membership: `exists` and `!exists`, with a literal, an `else deny`
    /// and an `else null` attribute, and `is null` in the filter.
    #[db]
    pub policy Members(who: &Who) for Doc {
        list: exists Member(doc_id == id, user_id == who.user_id else deny) || status == "published",
        view: !exists Block(blocked_id == author_id, blocker_id == who.user_id else deny) && status == "draft",
        create: exists Member(doc_id == id, role is null) || score > 1,
        update: exists Member(doc_id == id, user_id == who.user_id else deny, role == "editor"),
        update_check: exists Member(doc_id == id, user_id == who.user_id else null),
        delete: !exists Member(doc_id == id, user_id == who.user_id else deny) && status == "published",
    }

    /// `uuid` and `timestamptz` columns, against literals and attributes. No
    /// delete rule: no policy and no grant.
    #[db]
    pub policy Kinds(who: &Who) for Doc {
        list: team_id == who.team else deny || published_at <= who.since else deny || team_id == NIL,
        view: team_id is null && published_at > EPOCH,
        create: team_id == who.team else deny && published_at is not null,
        update: team_id == who.team else deny && published_at is null,
        update_check: team_id == who.team else deny,

        db {
            subjects: [
                Who::default(),
                Who::user(1),
                Who::team(1, 1, 0),
                Who::team(2, 9, -1_000_000),
            ],
            cases: 32,
        }
    }

    /// Write masks with no guard: column grants (decision 9). An update
    /// sets `status` and `score` only, so `update_check` sees the stored
    /// author.
    #[db]
    pub policy Masked(who: &Who) for Doc {
        list: author_id == who.user_id else deny || status == "published",
        create: author_id == who.user_id else deny,
        update: author_id == who.user_id else deny,
        update_check: author_id == who.user_id else deny && status != "archived",
        writable: only [status, score],
        writable_on_create: only [author_id, tenant_id, status, score],
    }

    /// One rule the database cannot check, and a mask that permits no
    /// column: the harness leaves `DELETE` and `UPDATE` out.
    #[db]
    pub policy AppOnly(who: &Who) for Doc {
        list: status == "published",
        update: status == "published",
        writable: none,
        #[app_only("reads the clock")]
        delete: { if who.admin { brocanth_policy::Pred::Always } else { brocanth_policy::Pred::Never } },
    }

    /// `update` reaches rows that `list` hides: the harness must fail.
    #[db]
    pub policy UpdateOutside(who: &Who) for Doc {
        list: status == "published",
        update: author_id == who.user_id else deny,
    }

    /// `update_check` admits a new row that `list` hides: the harness must
    /// fail.
    #[db]
    pub policy CheckOutside(who: &Who) for Doc {
        list: author_id == who.user_id else deny,
        update: author_id == who.user_id else deny,
        update_check: always,
    }

    /// The generated example's shape: an author edits a draft and may
    /// publish it, so `update_check` admits a draft or a published row.
    /// The mask has a guard.
    #[db]
    pub policy Publish(who: &Who) for Doc {
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        create: author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        update_check {
            who.admin => always,
            author_id == who.user_id else deny && status in ["draft", "published"],
        }
        writable {
            who.admin => except [id],
            only [score, status],
        }
    }

    /// `Publish` with `update_check` forgotten: the application lets an
    /// author publish a draft, and the database refuses it. The harness
    /// must fail.
    #[db]
    pub policy PublishNoCheck(who: &Who) for Doc {
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        create: author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        writable {
            who.admin => except [id],
            only [score, status],
        }
    }

    /// No `update_check`, and the mask permits no column `update` reads:
    /// no write leaves `update`, so there is nothing to report.
    #[db]
    pub policy NoCheckNeeded(who: &Who) for Doc {
        list: author_id == who.user_id else deny || status == "published",
        update: author_id == who.user_id else deny,
        writable: only [status, score],
    }
}

macro_rules! require_db {
    () => {
        match test_database_url() {
            Some(url) => url,
            None => {
                eprintln!("!! SKIPPED: BROCANTH_TEST_DATABASE_URL is not set");
                return;
            }
        }
    };
}

fn subjects() -> Vec<Who> {
    vec![
        Who::default(),
        Who::user(1),
        Who::user(2),
        Who::role(1, "editor"),
        Who::admin(),
        Who::banned(1),
        Who::team(1, 1, 0),
    ]
}

#[test]
fn literals_agree() {
    let url = require_db!();
    check::<Literals>(&url, &subjects(), 32).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn attributes_and_guard_chains_agree() {
    let url = require_db!();
    check::<Attrs>(&url, &subjects(), 32).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn exists_and_not_exists_agree() {
    let url = require_db!();
    check::<Members>(&url, &subjects(), 32).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn uuid_and_timestamptz_agree() {
    let url = require_db!();
    check::<Kinds>(&url, &subjects(), 32).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn unguarded_write_masks_agree() {
    let url = require_db!();
    check::<Masked>(&url, &subjects(), 32).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn an_app_only_command_is_left_out() {
    let url = require_db!();
    check::<AppOnly>(&url, &subjects(), 8).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn update_outside_list_fails() {
    let url = require_db!();
    let e = check::<UpdateOutside>(&url, &[Who::user(1)], 32).unwrap_err();
    assert!(
        e.contains("`update` admits rows that `list OR view` does not"),
        "{e}"
    );
    assert!(e.contains("help: keep `update` inside `list`"), "{e}");
}

#[test]
fn update_check_outside_list_fails() {
    let url = require_db!();
    let e = check::<CheckOutside>(&url, &[Who::user(1)], 32).unwrap_err();
    assert!(
        e.contains("`update_check` admits a new row that `list OR view` does not"),
        "{e}"
    );
    assert!(e.contains("help: keep `update_check` inside `list`"), "{e}");
}

fn authors() -> Vec<Who> {
    vec![Who::default(), Who::user(1), Who::user(2), Who::admin()]
}

#[test]
fn a_status_change_with_update_check_agrees() {
    let url = require_db!();
    check::<Publish>(&url, &authors(), 32).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_forgotten_update_check_fails() {
    let url = require_db!();
    let e = check::<PublishNoCheck>(&url, &authors(), 32).unwrap_err();
    assert!(
        e.contains("set `status` to") && e.contains("and the database refuses it"),
        "{e}"
    );
    assert!(e.contains("The policy has no `update_check`"), "{e}");
    assert!(e.contains("help: write `update_check`"), "{e}");
}

#[test]
fn no_update_check_and_no_writable_column_read_by_update_agrees() {
    let url = require_db!();
    check::<NoCheckNeeded>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn no_url_skips_and_passes() {
    // What `db { … }` does without a database.
    brocanth_rls::agreement::run::<Literals>(None, &subjects(), 1);
}
